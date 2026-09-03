/**
 * §79's upload — moving a sealed black box window to the Hub, and deciding what the far side's
 * answer means.
 *
 * The mirror of `bundle.ts`, and the whole of the difference is one sentence from §79.2:
 *
 * > §75's fetch has a rule that makes progress free: *"the partial's length is the progress"*. An
 * > upload cannot have that rule, because **the partial is on the other machine.**
 *
 * Three consequences, and they are the reason this is a separate file rather than a flag on
 * `planFetch`:
 *
 * 1. **There is a probe.** A decision this side cannot reach without asking, so `probe` is a
 *    decision rather than a step hidden inside a caller. An edition that folded it into "start"
 *    would re-send whole windows.
 * 2. **The offset is a claim, not evidence.** It places the next byte and it never concludes a
 *    transfer. That conclusion is the digest the receiver computed against the digest recorded at
 *    the seal, before the transfer existed (§77.1).
 * 3. **A failure cannot discard anything.** `checkBundle` deletes a file the robot owns;
 *    `checkUpload` can only stop believing an offset and start again at zero. The robot has no
 *    authority over the receiver's disk, and a contract that pretended otherwise would be one
 *    whose remedy nothing could carry out.
 *
 * # The size is known here, and it is not known there
 *
 * `planFetch` treats `size` as optional because a release manifest may not declare one. This
 * function does not: the window is a file this robot sealed and measured, so its length is a fact
 * rather than a claim, and the branch §12 leaves unexercised — *"a partial cannot be larger than
 * what it is part of"* — is reachable on every upload.
 *
 * # This decides, and does not transfer
 *
 * No network, no filesystem, no clock, exactly as `bundle.ts`.
 */

import type { Bearer } from './bundle.ts'

export const UPLOAD_DECISIONS = [
  'done',
  'probe',
  'start',
  'resume',
  'restart',
  'wait_for_unmetered',
  'no_bearer',
  'refuse',
] as const

export type UploadDecision = (typeof UPLOAD_DECISIONS)[number]

/**
 * What the receiver says it holds under this digest.
 *
 * `digest` is optional because a receiver holding a partial has nothing meaningful to hash — and
 * because a receiver holding a *complete* object and declining to say what it hashes to has given
 * this robot nothing it can act on, which §79.2 resolves by restarting rather than by assuming.
 */
export interface Held {
  bytes?: number
  digest?: string | null
}

/** The window, and what is known about the far side of the link (§79.2). */
export interface UploadSubject {
  /** `sha256:…` recorded at the seal. The address, and the only evidence in this structure. */
  digest?: string
  /** The window's length. A file this robot owns, so this is measured rather than declared. */
  size?: number
  /** What the receiver reported, or `null` where it has not been asked this pass. */
  held?: Held | null
  /** Whether a receipt for these exact bytes is already on the disk (§79.4). */
  receipt?: boolean
  /** The active bearer, when there is one. */
  bearer?: Bearer | null
}

export interface UploadPlan {
  decision: UploadDecision
  /** The byte offset to send from. Zero unless the decision is `resume`. */
  from: number
  detail: string
}

/**
 * What to do about one sealed window (§79, §34.3).
 *
 * The order of the tests is the order of cost, as in `planFetch`. Having a receipt is free;
 * refusing costs nothing; waiting for a link costs nothing; asking costs one small request; and
 * only then are megabytes considered.
 */
export function planUpload(subject: UploadSubject): UploadPlan {
  const digest = (subject.digest ?? '').trim()
  const size = Math.max(0, Math.trunc(subject.size ?? 0))

  if (subject.receipt === true) {
    return {
      decision: 'done',
      from: 0,
      detail: 'The receiver has acknowledged this window by digest and the receipt is on the '
        + 'disk. Nothing to send.',
    }
  }

  if (digest === '') {
    return {
      decision: 'refuse',
      from: 0,
      detail: 'This window carries no digest, so there is no address to send it to and nothing to '
        + 'check the far side\'s answer against. §79.1 makes the digest the route, and bytes with '
        + 'no name have nowhere to go.',
    }
  }

  if (size === 0) {
    return {
      decision: 'refuse',
      from: 0,
      detail: 'This window is empty. A seal that produced no bytes is a fact about the recorder '
        + 'worth reporting (§77.2) and is not something to upload — a zero-length object under a '
        + 'digest would be an artifact an investigator could open and mistake for the window.',
    }
  }

  const bearer = subject.bearer ?? null
  if (bearer === null) {
    return {
      decision: 'no_bearer',
      from: 0,
      detail: 'No bearer is up, so there is nothing to send over. That is the ordinary state for '
        + 'most of a robot\'s life (§34.1) and nothing is lost by it — the window is sealed on the '
        + 'disk and the incident naming it is already queued.',
    }
  }

  if (bearer.metered === true) {
    return {
      decision: 'wait_for_unmetered',
      from: 0,
      detail: `A sealed window is tens of megabytes and \`${bearer.name ?? '?'}\` is `
        + `${bearer.metering_declared === true ? 'declared' : 'assumed'} metered, so it waits for `
        + 'an unmetered link (§34.3). The alarm does not wait: §79.3 puts the incident on any '
        + 'bearer that is up, so an operator already knows this recording exists and what it '
        + 'hashes to.',
    }
  }

  const held = subject.held ?? null
  if (held === null) {
    return {
      decision: 'probe',
      from: 0,
      detail: 'The receiver has not been asked how much of this window it holds. §79.2: the '
        + 'partial is on the other machine, so an upload has a step a download does not — and a '
        + 'robot that skipped it would re-send the whole window every pass.',
    }
  }

  const bytes = Math.max(0, Math.trunc(held.bytes ?? 0))
  const heldDigest = (held.digest ?? '').trim()

  if (bytes >= size) {
    if (bytes === size && heldDigest !== '' && heldDigest === digest) {
      return {
        decision: 'done',
        from: 0,
        detail: `The receiver holds all ${size} bytes and says they hash to this window's digest. `
          + 'Nothing is sent and a receipt is written — which is the state a robot reaches after '
          + 'an upload whose acknowledgement was lost.',
      }
    }
    return {
      decision: 'restart',
      from: 0,
      detail: `The receiver claims ${bytes} bytes for a window of ${size}`
        + `${bytes === size
          ? ', and does not say what they hash to. A complete object nobody will name is not '
            + 'evidence this window arrived, and writing a receipt for it would record an upload '
            + 'that may never have happened'
          : '. A partial cannot be larger than what it is part of, so what is over there is not '
            + 'part of this window'}. `
        + 'It is sent again from the beginning.',
    }
  }

  if (bytes > 0) {
    return {
      decision: 'resume',
      from: bytes,
      detail: `Resuming at ${bytes} of ${size} bytes. The offset is the receiver's claim and is `
        + 'used to place the next byte, never to conclude the window arrived — that is the '
        + 'digest\'s job (§79.2).',
    }
  }

  return {
    decision: 'start',
    from: 0,
    detail: `Sending ${size} bytes over \`${bearer.name ?? '?'}\`.`,
  }
}

// ── What the receiver ended up with ──────────────────────────────────────────

export const UPLOAD_CHECKS = ['complete', 'short', 'long', 'digest_mismatch'] as const

export type UploadCheckCode = (typeof UPLOAD_CHECKS)[number]

/** What the receiver reported after the last byte was offered. */
export interface Reported {
  bytes?: number
  digest?: string | null
}

/** What this robot sealed, and measured. */
export interface Window {
  digest?: string
  size?: number
}

export interface UploadCheck {
  code: UploadCheckCode
  /** Whether a receipt may be written for this window. */
  ok: boolean
  /** Whether the next attempt must begin at zero rather than resume. */
  restart: boolean
  detail: string
}

/**
 * Did the window arrive (§79.2, §79.4)?
 *
 * The same four outcomes as `checkBundle` and one different remedy, which is the asymmetry worth
 * naming: a bundle that fails its check is **deleted**, because the robot owns those bytes. A
 * window that fails this check cannot be deleted from anywhere — the robot can only decline to
 * trust the offset it was given and begin again. `restart` is therefore about what this robot will
 * do next, never about what the receiver should do with what it has.
 *
 * Short keeps the offset, for `checkBundle`'s reason in the other direction: a transfer that
 * stopped early is what a link dropping looks like, and it is exactly what a resume is for.
 */
export function checkUpload(reported: Reported, window: Window): UploadCheck {
  const want = (window.digest ?? '').trim()
  const got = (reported.digest ?? '').trim()
  const size = Math.max(0, Math.trunc(window.size ?? 0))
  const bytes = Math.max(0, Math.trunc(reported.bytes ?? 0))

  if (bytes < size) {
    return {
      code: 'short',
      ok: false,
      restart: false,
      detail: `The receiver holds ${bytes} of ${size} bytes. The transfer stopped early, which is `
        + 'what a link dropping looks like, and the next pass resumes from what is there.',
    }
  }

  if (bytes > size) {
    return {
      code: 'long',
      ok: false,
      restart: true,
      detail: `The receiver holds ${bytes} bytes for a window of ${size}. More than was sent is `
        + 'not a transfer that stopped — it is another object under this digest, or a proxy '
        + 'answering for one. The next attempt begins at zero; this robot cannot delete what is '
        + 'over there and does not pretend to.',
    }
  }

  if (want === '' || got === '' || want !== got) {
    return {
      code: 'digest_mismatch',
      ok: false,
      restart: true,
      detail: 'The receiver holds the right number of bytes and they are not this window\'s. The '
        + 'digest was recorded at the seal, before any transfer existed (§77.1), so it is the '
        + 'half of this comparison that is evidence. No receipt is written and the next attempt '
        + 'begins at zero.',
    }
  }

  return {
    code: 'complete',
    ok: true,
    restart: false,
    detail: 'The receiver holds the whole window and it hashes to what was sealed. A receipt is '
      + 'written beside the recording; the recording itself is kept, because §39.1\'s local route '
      + 'is for the operator standing next to the robot (§79.4).',
  }
}
