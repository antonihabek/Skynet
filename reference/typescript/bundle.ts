/**
 * §12's fetch — deciding whether to download an OS image, from where, and whether what arrived is
 * the image.
 *
 * §12 gates *installing* an image and says nothing about getting one, because getting one looks
 * like an implementation detail. It is not. Three of the rules in this file exist because a
 * hundred-megabyte transfer over a link somebody pays for, to a machine in a warehouse, is a
 * different problem from a JSON `GET`:
 *
 * 1. **§34.3's metering.** *"Bulk transfer respects metering […] pushing those over a metered link
 *    because it happened to be the only one up is how a fleet generates a surprise invoice."* An
 *    OS image is the largest bulk transfer this system has, so it always waits for an unmetered
 *    bearer, and the wait is reported rather than silent.
 * 2. **A partial download is bound to the digest it is a partial of.** This is §12's own rule about
 *    a bench pass — *"evidence that is not bound to what it is evidence of is not evidence"* —
 *    applied to bytes. A partial keyed only by version would let a resume append one release's tail
 *    to another's head, and the failure would surface as a digest mismatch after the whole transfer
 *    rather than before it.
 * 3. **What arrived is checked against the manifest, and a failure deletes it.** A bundle that does
 *    not hash to what the signed manifest says is not the image; keeping it means the next resume
 *    appends to bad bytes for ever, and it leaves an artifact on disk that some future code path
 *    will find and treat as one.
 *
 * # This decides, and does not transfer
 *
 * No network, no filesystem, no clock. The bearer's facts arrive as values, the size of whatever is
 * on the disk arrives as a number, and the digest of what arrived arrives already computed — so the
 * same function runs on the robot and on a Hub asking whether a robot could have fetched an image.
 */

export const FETCH_DECISIONS = [
  'have',
  'fetch',
  'resume',
  'discard_and_fetch',
  'wait_for_unmetered',
  'no_bearer',
  'refuse',
] as const

export type FetchDecision = (typeof FETCH_DECISIONS)[number]

/** What the robot knows about the link it would download over (§34.3). */
export interface Bearer {
  name?: string
  /** Whether it charges by the megabyte. Cellular is assumed to unless declared otherwise. */
  metered?: boolean
  /** Whether the metering was declared rather than assumed, for the sentence. */
  metering_declared?: boolean
}

/** The image, and what is already on the disk for it. */
export interface FetchSubject {
  /** `sha256:…` from the signed release manifest. What the bytes must hash to. */
  digest?: string
  /** The size the manifest declares, when it declares one. */
  size?: number | null
  /** Whether a complete, verified bundle for this digest is already here. */
  have?: boolean
  /** Bytes of a partial download **keyed to this digest**. */
  partial?: number
  /** The active bearer, when there is one. */
  bearer?: Bearer | null
}

export interface FetchPlan {
  decision: FetchDecision
  /** The byte offset to start from. Zero unless the decision is `resume`. */
  from: number
  detail: string
}

/**
 * What to do about one OS image (§12, §34.3).
 *
 * The order of the tests is the order of cost. Having it already is free; refusing costs nothing;
 * waiting for a link costs nothing; and only then is a transfer considered. A version that checked
 * metering first would report a robot as waiting for an unmetered bearer to fetch an image it
 * already has.
 */
export function planFetch(subject: FetchSubject): FetchPlan {
  const digest = (subject.digest ?? '').trim()
  const partial = Math.max(0, Math.trunc(subject.partial ?? 0))
  const size = subject.size ?? null

  if (subject.have === true) {
    return {
      decision: 'have',
      from: 0,
      detail: 'This bundle is already on the disk and has been checked against the manifest. '
        + 'Nothing to fetch.',
    }
  }

  if (digest === '') {
    return {
      decision: 'refuse',
      from: 0,
      detail: 'This release names no bundle digest, so there is nothing to check the bytes '
        + 'against. §12 binds a bench pass to the exact bytes it was taken against, and bytes with '
        + 'no name cannot be bound to — so they are not fetched either.',
    }
  }

  const bearer = subject.bearer ?? null
  if (bearer === null) {
    return {
      decision: 'no_bearer',
      from: 0,
      detail: 'No bearer is up, so there is nothing to fetch over. That is the ordinary state for '
        + 'most of a robot’s life (§34.1) and nothing about the update is lost by it — '
        + 'the release manifest is already here and the transfer resumes when a link returns.',
    }
  }

  if (bearer.metered === true) {
    return {
      decision: 'wait_for_unmetered',
      from: 0,
      detail: `An OS image is the largest bulk transfer this system has, and \`${bearer.name ?? '?'}\` `
        + `is ${bearer.metering_declared === true ? 'declared' : 'assumed'} metered. It waits for an `
        + 'unmetered link (§34.3). Nothing is lost by waiting, and a fleet that downloaded '
        + 'hundreds of megabytes per robot over cellular would find out at the end of the month.',
    }
  }

  // A partial at least as large as the whole thing is not a partial of this bundle. Resuming from
  // it would append this image's tail to something else's, and the mismatch would only surface
  // after the entire transfer — which on a poor link is hours.
  if (size !== null && Number.isFinite(size) && size > 0 && partial >= size) {
    return {
      decision: 'discard_and_fetch',
      from: 0,
      detail: `There are ${partial} bytes here for a bundle the manifest says is ${size}. A partial `
        + 'cannot be larger than what it is part of, so this is left over from something else. It '
        + 'is discarded and the transfer starts again.',
    }
  }

  if (partial > 0) {
    return {
      decision: 'resume',
      from: partial,
      detail: `Resuming at ${partial} bytes${size === null ? '' : ` of ${size}`}. The partial is `
        + 'keyed to this bundle’s digest, so it cannot be a different image’s tail.',
    }
  }

  return {
    decision: 'fetch',
    from: 0,
    detail: `Fetching ${size === null ? 'the bundle' : `${size} bytes`} over \`${bearer.name ?? '?'}\`.`,
  }
}

// ── What arrived ─────────────────────────────────────────────────────────────

export const BUNDLE_CHECKS = ['match', 'short', 'long', 'digest_mismatch'] as const

export type BundleCheckCode = (typeof BUNDLE_CHECKS)[number]

export interface Arrived {
  /** `sha256:…` over the bytes that are on the disk. */
  digest?: string
  size?: number
}

export interface Expected {
  digest?: string
  size?: number | null
}

export interface BundleCheck {
  code: BundleCheckCode
  /** Whether these bytes may be installed. */
  ok: boolean
  /** Whether the file should be deleted rather than kept as a partial. */
  discard: boolean
  detail: string
}

/**
 * Are these the bytes the signed manifest describes (§12)?
 *
 * Size is checked before the digest, and the reason is the sentence rather than the verdict — both
 * refuse. A file short of its declared size is a transfer that stopped, which is ordinary and
 * resumable; a file of exactly the right size whose digest is wrong is a different set of bytes,
 * which is a build somebody re-cut or a transfer somebody interfered with. Reporting the first as a
 * digest mismatch would send an engineer looking for tampering every time a link dropped.
 *
 * # A failure discards, except the one that does not
 *
 * `short` keeps the file, because that is exactly what a resume is for. Everything else deletes it:
 * bytes that will never hash correctly are bytes a later resume would append to for ever, and an
 * unverified bundle left on the disk is an artifact some future code path will find.
 */
export function checkBundle(arrived: Arrived, expected: Expected): BundleCheck {
  const want = (expected.digest ?? '').trim()
  const got = (arrived.digest ?? '').trim()
  const size = expected.size ?? null
  const actual = arrived.size ?? 0

  if (size !== null && Number.isFinite(size) && actual < size) {
    return {
      code: 'short',
      ok: false,
      discard: false,
      detail: `${actual} of ${size} bytes arrived. The transfer stopped early, which is what a link `
        + 'dropping looks like. The partial is kept and the next attempt resumes from it.',
    }
  }

  if (size !== null && Number.isFinite(size) && actual > size) {
    return {
      code: 'long',
      ok: false,
      discard: true,
      detail: `${actual} bytes arrived for a bundle the manifest says is ${size}. More than was `
        + 'declared is not a transfer that stopped — it is a different artifact, or a proxy that '
        + 'served something else. Discarded.',
    }
  }

  if (want === '' || got === '' || want !== got) {
    return {
      code: 'digest_mismatch',
      ok: false,
      discard: true,
      detail: 'These bytes do not hash to what the signed release manifest says this bundle is. '
        + '§12 binds every gate it applies to that digest, so bytes that are not it are not '
        + 'this release — do not install them, and find out where they came from. Discarded, '
        + 'because bytes that will never verify are bytes a resume would append to for ever.',
    }
  }

  return {
    code: 'match',
    ok: true,
    discard: false,
    detail: 'These bytes are the release the manifest describes.',
  }
}
