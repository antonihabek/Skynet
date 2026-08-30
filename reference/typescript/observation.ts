/**
 * Observation assembly (§25.1) — the TypeScript oracle.
 *
 * §25.1 calls this “one of the harder parts of the whole system”, and the reason is a sentence
 * worth keeping in front of you while reading the rest of this file:
 *
 * > A policy is trained on synchronised frames: image and joint state as of one instant. A real
 * > robot has three cameras at 30 Hz with 20–60 ms of pipeline latency, joint state at 1 kHz with
 * > 1 ms, and an IMU at 200 Hz — none of them aligned, none of them stamped the same way. Feed a
 * > policy the most recent value from each and it receives **a frame that never existed**, in which
 * > the arm is where it is *now* and the image shows where it was two frames ago. Policies degrade
 * > in ways that look like bad training.
 *
 * The last clause is the expensive part. A skewed frame does not announce itself: it presents as a
 * policy that grasps slightly late, or oscillates on contact, or works on the bench and not on the
 * robot — every one of which sends somebody to retrain a model that was fine.
 *
 * This module does not synchronise anything, and §25.1 is explicit that it should not try: nothing
 * here can make an unsynchronised robot into a synchronised one. What it does is make the skew
 * **measured and visible** instead of silently folded into the policy's behaviour.
 *
 * # Arithmetic, and the one boundary worth naming
 *
 * `crates/skynet-contracts/src/observation.rs` computes the same function in `i128`, exactly, at
 * any stamp a `u64` can hold. This edition computes it in `number`, which is exact for integers
 * below 2^53 — about 104 days of monotonic uptime in nanoseconds — and inexact above it.
 *
 * That is not a divergence the corpus can hide, because it is not a divergence the corpus can
 * *reach*: `JSON.parse` has already rounded any stamp above 2^53 before this code sees it, so no
 * case can carry one. It is stated here rather than left to be discovered, and it costs nothing,
 * because §31.3 puts this edition off-robot: the TypeScript edition is the Hub, the workstation CLI
 * and this oracle, and none of them assembles a frame on a machine that has been up for four
 * months.
 */

/** One sample from one stream. */
export interface Sample {
  /** `CLOCK_MONOTONIC_RAW` at **acquisition**, never at return or arrival (§24, §47.2). */
  stamp_ns: number
  /** The capture sequence number, so a recorded frame can be tied back to its buffer. */
  seq: number
}

/** One stream the policy asked for, and what is available on it. */
export interface Stream {
  /** The sensor's name on the embodiment descriptor. */
  name: string
  /** Its modality, so a hold can say “the depth stream is stale” rather than “stream 3 is stale”. */
  modality: string
  /**
   * The declared per-sensor latency (§52), in milliseconds: how far behind the world this stream's
   * timestamps are. Absent means nobody declared it — neither a failure nor a zero.
   */
  latency_ms?: number | null
  /**
   * How wrong `latency_ms` could be, in milliseconds (§47.2). Supplied by the capture pipeline, not
   * by the descriptor: a backend that stamps at capture knows the figure precisely, and one that
   * estimates knows it badly.
   */
  uncertainty_ms?: number | null
  /**
   * How old a sample may be before this stream is unusable, from the policy's declared observation
   * requirements. The same camera is fine at 200 ms for a mapping policy and useless at 40 ms for a
   * contact-rich manipulation one.
   */
  budget_ms?: number | null
  /** Every sample available, in any order. */
  samples: Sample[]
}

/** The sample the assembler picked, and what accepting it cost. */
export interface Chosen {
  stamp_ns: number
  seq: number
  /** The instant the sample actually describes: its stamp, less the declared latency. */
  effective_ns: number
  /**
   * How far behind the target instant that is, in milliseconds. **Signed** — negative means the
   * sample describes an instant after the one the frame claims to be, which cannot happen live and
   * happens routinely in replay.
   */
  age_ms: number
  over_budget: boolean
  /** How many samples were available to choose from. */
  considered: number
}

/** Why a frame cannot be delivered. */
export interface Hold {
  stream: string
  /** `no_sample` or `stale`. */
  reason: 'no_sample' | 'stale'
  detail: string
}

/** What one stream contributed. */
export interface StreamFrame {
  name: string
  modality: string
  latency_ms: number | null
  uncertainty_ms: number | null
  budget_ms: number | null
  chosen: Chosen | null
}

/** One assembled observation frame. */
export interface Frame {
  target_ns: number
  streams: StreamFrame[]
  /**
   * The spread between the earliest and latest instants actually in the frame. Null when fewer than
   * two streams contributed: the spread of one instant is not zero, it is undefined, and zero would
   * let a half-empty frame look perfectly synchronised.
   */
  skew_ms: number | null
  /**
   * The spread once each stream's uncertainty is taken at its worst. Null when any latency is
   * undeclared: the widening is then unbounded, and §47.2's “larger uncertainty” has no largest.
   */
  skew_upper_ms: number | null
  undeclared_latency: string[]
  undeclared_budget: string[]
  holds: Hold[]
  verdict: 'ok' | 'hold'
  deliverable: boolean
}

/**
 * One assembled frame, projected onto what §26's `/sk/frame` record can carry.
 *
 * §25.1's last bullet: “The frame's skew and per-stream ages go into the black box, so ‘the policy
 * behaved oddly’ can be checked against ‘the frame it was given was 80 ms skewed’ months later.”
 * The record exists — channel 5, `sk_frame` — and this is the projection onto it.
 *
 * A projection rather than a rename, and it loses two things which are stated here rather than
 * discovered by whoever reads a recording:
 *
 * * **`age_us` cannot be negative.** The record's field is unsigned, so a chosen sample from *after*
 *   the target instant saturates at zero. It cannot arise on a live robot — every target a runtime
 *   picks is at or before now, and `CLOCK_MONOTONIC_RAW` does not step — which is why the saturation
 *   is acceptable rather than merely convenient. The signed figure stays on the frame.
 * * **`worst_skew_us` is null when the skew is not measurable.** `skew_ms` is null when fewer than
 *   two streams contributed, because the spread of one instant is undefined. The record format
 *   carries it — `SKEW_UNMEASURABLE` in `blackbox.ts`, added in format version 2 for this — because
 *   zero would make a frame with one working stream and three dead capture paths read, months later,
 *   as perfectly synchronised.
 */
export interface Record {
  target_ns: number
  worst_skew_us: number | null
  /**
   * Streams that were declared and produced no sample.
   *
   * Distinct from the record's `streams_omitted`, which is “streams the record could not fit” — a
   * limit of the format. This is a limit of the robot, and an investigation reading one as the other
   * would be looking for a bigger record instead of a dead camera. Version 1 of the record had no
   * field for it at all, which is one of the two reasons there is a version 2.
   */
  streams_absent: number
  streams: { name: string, age_us: number, stale: boolean }[]
}

/** Project a frame onto what the black box records (§25.1, §26). */
export function toRecord(frame: Frame): Record {
  const streams: Record['streams'] = []
  let absent = 0
  for (const stream of frame.streams) {
    if (stream.chosen === null) {
      absent += 1
      continue
    }
    streams.push({
      name: stream.name,
      age_us: micros(stream.chosen.age_ms),
      stale: stream.chosen.over_budget,
    })
  }
  return {
    target_ns: frame.target_ns,
    worst_skew_us: frame.skew_ms === null ? null : micros(frame.skew_ms),
    streams_absent: absent,
    streams,
  }
}

/**
 * Milliseconds as whole microseconds, saturating at zero and at the top of a `u32`.
 *
 * The ceiling is not a real case — seventy-one minutes of skew — and it is here because the Rust
 * edition's alternative in a release build is a wrap, and a frame recorded as four microseconds
 * skewed when it was four thousand seconds skewed is worse than one recorded at the ceiling.
 */
function micros(ms: number): number {
  const value = roundHalfAway(ms * 1000)
  if (value <= 0) return 0
  if (value >= 4_294_967_295) return 4_294_967_295
  return value
}

const NS_PER_MS = 1_000_000

/**
 * Round half away from zero.
 *
 * **Not `Math.round`**, which rounds half toward +∞ — `Math.round(-0.5)` is `-0`. The Rust edition
 * cannot use `f64::round` either, because it is unavailable under `no_std` without a maths library,
 * and §31.2's rule about platform maths applies with full force to a rounding inside a latency
 * correction: two editions rounding a half-nanosecond differently is a one-nanosecond disagreement
 * today and a different chosen sample the day two candidates are equidistant. So both editions
 * write the same four lines out.
 */
function roundHalfAway(value: number): number {
  if (!Number.isFinite(value)) return 0
  const truncated = Math.trunc(value)
  const fraction = value - truncated
  if (fraction >= 0.5) return truncated + 1
  if (fraction <= -0.5) return truncated - 1
  return truncated
}

/** The instant a sample describes: its stamp, less the declared latency, in nanoseconds. */
function effectiveNs(stampNs: number, latencyMs: number | null | undefined): number {
  const latencyNs = latencyMs === undefined || latencyMs === null
    ? 0
    : roundHalfAway(latencyMs * NS_PER_MS)
  return stampNs - latencyNs
}

/**
 * Nanoseconds as milliseconds. One division of an exact integer, so every edition performing it
 * gets the same bits: IEEE-754 division is correctly rounded everywhere, which is the property
 * §31.2 leans on for the kernel's `sin` and `cos`.
 */
function ms(nanos: number): number {
  return nanos / NS_PER_MS
}

/**
 * Assemble one observation frame for the given instant (§25.1).
 *
 * For each stream: correct every sample for the declared latency, pick the one nearest the target,
 * and measure how far behind the target it is. Then measure the spread across the streams, which is
 * the frame's own internal inconsistency.
 *
 * # Nearest, and what happens on a tie
 *
 * Nearest is by absolute distance, so a sample *after* the target can be chosen when it is
 * genuinely closer — impossible on a live robot, correct in replay.
 *
 * A tie takes the **earlier** sample. Two samples equidistant either side of the target are equally
 * good by the distance rule and are not equally good in any other sense: the earlier describes
 * something that had already happened at the instant the frame claims to describe, and the later
 * does not. The tie-break also has to exist for a duller reason — without it, two editions
 * iterating in different orders would pick different samples and the corpus would diverge on an
 * input where nothing is wrong.
 */
export function assembleObservation(targetNs: number, streams: Stream[]): Frame {
  const frames: StreamFrame[] = []
  const holds: Hold[] = []
  const undeclaredLatency: string[] = []
  const undeclaredBudget: string[] = []

  for (const stream of streams) {
    const latency = stream.latency_ms ?? null
    const uncertainty = stream.uncertainty_ms ?? null
    const budget = stream.budget_ms ?? null
    if (latency === null) undeclaredLatency.push(stream.name)
    if (budget === null) undeclaredBudget.push(stream.name)

    let best: { effective: number, sample: Sample } | null = null
    for (const sample of stream.samples) {
      const effective = effectiveNs(sample.stamp_ns, latency)
      const distance = Math.abs(effective - targetNs)
      let better: boolean
      if (best === null) {
        better = true
      } else {
        const bestDistance = Math.abs(best.effective - targetNs)
        // Strictly nearer wins; on a tie the earlier instant wins. Never “the one seen first”,
        // which would make the answer depend on the order the capture ring happened to be walked.
        better = distance < bestDistance
          || (distance === bestDistance && effective < best.effective)
      }
      if (better) best = { effective, sample }
    }

    let chosen: Chosen | null = null
    if (best === null) {
      holds.push({
        stream: stream.name,
        reason: 'no_sample',
        detail: `${stream.name} (${stream.modality}) offered no sample for this instant, so there `
          + 'is no frame to assemble. A stream the policy declared and the pipeline is not '
          + 'producing is a capture path that is not running.',
      })
    } else {
      const age = ms(targetNs - best.effective)
      // One-sided, and deliberately so. The budget bounds how far *behind* the target a sample may
      // be; a sample from ahead of it is a clock fault rather than a stale one, it is visible in
      // the signed age, and reporting it as staleness would send somebody to look at a capture
      // pipeline that is working.
      const overBudget = budget !== null && age > budget
      if (overBudget) {
        holds.push({
          stream: stream.name,
          reason: 'stale',
          detail: `${stream.name} (${stream.modality}) is ${age} ms behind the frame's `
            + `instant and the policy's budget for it is ${budget ?? 0} ms. §25.1: `
            + 'exceeding it is a hold, not a silently older frame.',
        })
      }
      chosen = {
        stamp_ns: best.sample.stamp_ns,
        seq: best.sample.seq,
        effective_ns: best.effective,
        age_ms: age,
        over_budget: overBudget,
        considered: stream.samples.length,
      }
    }

    frames.push({
      name: stream.name,
      modality: stream.modality,
      latency_ms: latency,
      uncertainty_ms: uncertainty,
      budget_ms: budget,
      chosen,
    })
  }

  const { skew, upper } = measureSkew(frames, undeclaredLatency)

  return {
    target_ns: targetNs,
    streams: frames,
    skew_ms: skew,
    skew_upper_ms: upper,
    undeclared_latency: undeclaredLatency,
    undeclared_budget: undeclaredBudget,
    holds,
    // §25.1: exceeding a staleness budget “is a hold, not a silently older frame”. So is an empty
    // stream. Everything else — an undeclared latency, an undeclared budget, an unbounded skew — is
    // reported and does not block, because none of them is evidence that the frame is wrong. They
    // are evidence that nobody can say how wrong it is, which is a different sentence.
    verdict: holds.length === 0 ? 'ok' : 'hold',
    deliverable: holds.length === 0,
  }
}

function measureSkew(
  frames: StreamFrame[], undeclaredLatency: string[],
): { skew: number | null, upper: number | null } {
  let lowest: { instant: number, uncertainty: number } | null = null
  let highest: { instant: number, uncertainty: number } | null = null
  let contributors = 0

  for (const frame of frames) {
    if (frame.chosen === null) continue
    contributors += 1
    const uncertainty = frame.uncertainty_ms ?? 0
    if (lowest === null || frame.chosen.effective_ns < lowest.instant) {
      lowest = { instant: frame.chosen.effective_ns, uncertainty }
    }
    if (highest === null || frame.chosen.effective_ns > highest.instant) {
      highest = { instant: frame.chosen.effective_ns, uncertainty }
    }
  }

  // One instant has no spread, and reporting zero would let a frame with a single working stream
  // look perfectly synchronised — the presentation §25.1 is written against.
  if (contributors < 2 || lowest === null || highest === null) return { skew: null, upper: null }

  const measured = ms(highest.instant - lowest.instant)
  if (undeclaredLatency.length > 0) return { skew: measured, upper: null }
  return { skew: measured, upper: measured + lowest.uncertainty + highest.uncertainty }
}

/**
 * The most recent instant at which every stream has a sample (§25.1's “targets an instant”).
 *
 * A live control loop has to choose a target, and choosing badly defeats the whole section: aim at
 * *now* and the slowest camera is always the stalest stream in every frame, so every frame is
 * skewed by that camera's latency and the assembler reports a problem the loop created. Aim here
 * and the frame is the most recent one that every stream can actually answer for.
 *
 * Null when any stream has no sample at or before the given instant — the same fact
 * `assembleObservation` reports as a `no_sample` hold, and worth having before building a frame
 * nobody can use.
 *
 * Offered rather than imposed: a runtime may target an older instant deliberately, and nothing here
 * prevents it. What this exists to prevent is two editions inventing two different defaults and
 * disagreeing about frames neither of them assembled wrongly.
 */
export function latestCommonInstant(nowNs: number, streams: Stream[]): number | null {
  let earliest: number | null = null

  for (const stream of streams) {
    let latest: number | null = null
    for (const sample of stream.samples) {
      const effective = effectiveNs(sample.stamp_ns, stream.latency_ms ?? null)
      if (effective > nowNs) continue
      if (latest === null || effective > latest) latest = effective
    }
    // A stream with nothing at or before `now` cannot contribute to any frame at or before `now`,
    // so there is no common instant at all — not a later one, and not one that quietly drops it.
    if (latest === null) return null
    if (earliest === null || latest < earliest) earliest = latest
  }

  // No streams at all: every instant is common to all of them.
  const answer = earliest === null ? nowNs : earliest
  // A declared latency larger than the stamp itself. The correction is still right and the instant
  // it names is before the monotonic clock started, which no `u64` can express.
  return answer < 0 ? null : answer
}
