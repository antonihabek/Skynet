/**
 * §11's pull — what a robot does with a desired state it has just been handed.
 *
 * > **The Hub stores intent. The robot reconciles.** Never push.
 * >
 * > `GET /devices/{id}/desired` → `{ os, packages, policy, profile, generation: 41 }`
 *
 * The document arrives over a network, which makes it the least trusted input in the system that is
 * also the most consequential: it decides what software runs on a machine that moves. §33.8 already
 * says what a robot does with a *corrupt file on its own disk* — last-known-good, then run nothing,
 * and it never invents one. This module answers the question one step earlier: **may this response
 * become that file at all?**
 *
 * # Refusing is not the same as ignoring
 *
 * Two of the four findings below refuse the offer, and in both cases the robot keeps reconciling
 * against what it already has. That is the correct outcome and not a degraded one — §34.1 assumes
 * the robot is offline most of the time, so "carry on with the last good intent" is the *ordinary*
 * state rather than a failure mode. A refused offer costs nothing that being offline does not.
 *
 * # Why a reused generation is refused rather than adopted
 *
 * §11 rests one property on the counter: *"the `generation` counter makes it idempotent."* A Hub
 * that serves two different documents under one generation has broken that, and the robot is the
 * only party that can see it — the Hub has no way to know the robot already applied a different 41.
 *
 * The two ways to handle it are not symmetric:
 *
 * * **Adopt**, and the robot runs the new content while reporting generation 41 as applied. A fleet
 *   console then shows robots converged on 41 that are running different software, which is exactly
 *   the claim §17 forbids the platform from making — and it is invisible, because every robot looks
 *   right.
 * * **Refuse**, and the robot keeps the old content and reports a fault naming the Hub's mistake. A
 *   console shows a robot that is not adopting and says why; an operator bumps the generation.
 *
 * One produces a visible, actionable state and the other produces an invisible wrong one. So it
 * refuses.
 *
 * # And why a *lower* generation is adopted
 *
 * The opposite call, and for a reason worth writing down. A generation that goes backwards is a Hub
 * restored from a backup, and a robot that refused to follow one would be a fleet an operator could
 * not recover — permanently, with no remedy short of visiting every machine. The replay it would
 * otherwise guard against is already the thing the credential and TLS prevent. So it is adopted,
 * and reported, because *"intent went backwards"* is a sentence somebody wants to see once.
 *
 * # Pure
 *
 * No network, no filesystem. Whether the document decoded arrives decided, the way a signature state
 * does in §37.1, so the same function runs on the robot and on a Hub checking what it just served.
 */

export const OFFER_FAULTS = [
  'undecodable',
  'generation_reused',
  'generation_went_backwards',
  'empties_the_robot',
] as const

export type OfferFaultCode = (typeof OFFER_FAULTS)[number]

export interface OfferFault {
  code: OfferFaultCode
  severity: 'error' | 'warning'
  message: string
}

/** A desired-state document the robot has just been handed. */
export interface Offer {
  /**
   * Whether it decoded. Arrives decided, because decoding is `DesiredState::from_json`'s job and a
   * second implementation of it here would be a second answer to a question already answered.
   */
  decoded?: boolean
  /** Why it did not, when it did not. */
  decode_error?: string
  generation?: number
  /** `sha256:…` over the document's canonical form. What distinguishes two documents. */
  digest?: string
  /** How many packages it names. */
  packages?: number
}

/** What the robot is reconciling against now. */
export interface Held {
  generation?: number
  digest?: string
  /** How many packages the robot is actually running. */
  running?: number
}

export interface OfferVerdict {
  /** Whether this document may become the desired state on disk. */
  adopt: boolean
  /**
   * Whether adopting it changes anything.
   *
   * False for the ordinary poll, which is most of them: §11's loop asks the same question every
   * few seconds and the answer is usually the document it already has. Separated from `adopt` so a
   * caller can skip the write without having to compare digests itself — and so that a log line
   * exists for "the Hub answered and nothing changed", which is different from "the Hub did not
   * answer".
   */
  changed: boolean
  /** The generation the robot would be reconciling towards. Its own when the offer is refused. */
  generation: number
  faults: OfferFault[]
}

function fault(code: OfferFaultCode, severity: 'error' | 'warning', message: string): OfferFault {
  return { code, severity, message }
}

/**
 * May this response become the desired state (§11, §17, §33.8)?
 *
 * Every check runs and the result carries all of them, for `admitPackage`'s reason: a fleet
 * operator diagnosing why a robot will not take an update should get one list rather than one
 * problem per poll.
 */
export function assessOffer(offer: Offer, held: Held): OfferVerdict {
  const faults: OfferFault[] = []
  const heldGeneration = held.generation ?? 0
  const heldDigest = (held.digest ?? '').trim()
  const offered = offer.generation ?? 0
  const digest = (offer.digest ?? '').trim()

  if (offer.decoded !== true) {
    const why = (offer.decode_error ?? '').trim()
    faults.push(fault('undecodable', 'error',
      'This response is not a desired state'
      + (why === '' ? '' : `: ${why}`)
      + '. It has not been written to disk, and this robot carries on reconciling against what it '
      + 'already has — which is what it does whenever it cannot reach the Hub at all (§34.1), so '
      + 'nothing about the robot is worse for it.'))
    return { adopt: false, changed: false, generation: heldGeneration, faults }
  }

  // The reused generation, and the reason it refuses. See the header.
  if (offered === heldGeneration && digest !== '' && heldDigest !== '' && digest !== heldDigest) {
    faults.push(fault('generation_reused', 'error',
      `The Hub is serving a different document under generation ${offered}, which this robot has `
      + 'already applied. §11 rests idempotence on that counter, so adopting this would have the '
      + 'robot running new software while reporting the old generation as applied — and a fleet '
      + 'console would show it converged. Bump the generation on the Hub and this robot will take '
      + 'it on the next poll.'))
    return { adopt: false, changed: false, generation: heldGeneration, faults }
  }

  if (offered < heldGeneration) {
    faults.push(fault('generation_went_backwards', 'warning',
      `This robot has applied generation ${heldGeneration} and the Hub is asking for ${offered}. `
      + 'Intent has gone backwards, which is what a Hub restored from a backup looks like. It is '
      + 'adopted — a robot that refused to follow one would be a fleet nobody could recover — and '
      + 'it is reported, because it is worth somebody knowing once.'))
  }

  const running = held.running ?? 0
  if ((offer.packages ?? 0) === 0 && running > 0) {
    faults.push(fault('empties_the_robot', 'warning',
      `This desired state names no packages and this robot is running ${running}. That is what an `
      + 'operator emptying a robot looks like, and it is also what a Hub serving a stub looks like. '
      + 'It is adopted, because stopping is never the dangerous direction (§33.4) — and it is said '
      + 'out loud, because the two are indistinguishable from here.'))
  }

  return {
    adopt: true,
    changed: digest !== heldDigest,
    generation: offered,
    faults,
  }
}
