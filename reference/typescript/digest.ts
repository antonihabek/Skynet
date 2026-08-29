/**
 * Version digests, the TypeScript half.
 *
 * A deliberate reimplementation of `crates/skynet-contracts/src/digest.rs`, for the reason
 * `canonical.ts` is one: a corpus whose expected values were produced by the code that checks them
 * proves nothing. The rule is in the specification and in both files, and the corpus is where the
 * two meet.
 *
 * The digest itself is the Hub's `computeVersionDigest`, which has identified published versions
 * since before the OS existed:
 *
 *     sha256(
 *       "skynet.version.v1" \n
 *       kind                \n
 *       version             \n
 *       canonical(manifest) \n
 *       "<path>\0<sha256>" \n … per file, sorted by path
 *     )
 *
 * The separator between a path and its hash is a NUL rather than a space: it is the one byte a
 * path may not contain, which `checkPath` enforces, and that is what makes the framing unambiguous
 * rather than merely unlikely to collide. It is written here as an escape rather than as the byte
 * itself, because the Hub's copy holds the raw byte and a terminal renders it as nothing — which is
 * how this implementation came to be written against a space and found out from a digest that did
 * not match.
 *
 * There is one deliberate departure and one deliberate restriction, both aimed at the same thing:
 * that a robot and the Hub either compute the same identity or neither computes one.
 *
 * **The departure.** The manifest is canonicalised with this repository's `canonical()` rather than
 * the Hub's `canonicalJson`. The two differ only in how they write a number whose magnitude is at
 * or above 1e21 or below 1e-6, where `JSON.stringify` reaches for an exponent and §31.4's rule
 * forbids one.
 *
 * **The restriction.** Rather than leave that difference to chance, a manifest carrying such a
 * number is refused. So is one whose object *keys* carry a character above U+FFFF, because the Hub
 * orders keys by UTF-16 code unit and the canonical rule orders them by UTF-8 byte, and those two
 * orders agree for every key in the basic multilingual plane and can disagree above it.
 *
 * Refusal rather than a best guess is the same discipline the rest of the platform applies to a
 * missing envelope: a package that does not install and a person reading a reason is a better
 * outcome than a package whose identity is one thing on the robot and another in the registry.
 */

import { createHash } from 'node:crypto'

import { canonical } from './canonical.ts'

/** Present so a digest over a version cannot collide with a digest over anything else. */
export const VERSION_DIGEST_DOMAIN = 'skynet.version.v1'

const JS_MIN_PLAIN = 1e-6
const JS_MAX_PLAIN = 1e21

export interface DigestFile {
  /** Path relative to the package root, `/` separated. */
  path: string
  /** Lowercase hex sha256 of the file's bytes. */
  sha256: string
}

/** Why a digest could not be computed. Each names the input rather than saying "failed". */
export type DigestError =
  | { kind: 'path'; path: string; detail: string }
  | { kind: 'duplicate_path'; path: string }
  | { kind: 'sha256'; path: string; value: string }
  | { kind: 'manifest'; detail: string }
  | { kind: 'number_out_of_range'; field: string; value: number }
  | { kind: 'key_outside_bmp'; at: string; key: string }

export type DigestOutcome =
  | { ok: true; digest: string }
  | { ok: false; error: DigestError }

/** Whether a string is a lowercase hex sha256. */
export function isHexSha256(value: string): boolean {
  return /^[0-9a-f]{64}$/.test(value)
}

/**
 * Whether a relative path may be used as one.
 *
 * The file list is both what is hashed and what is written to disk, and every rule here closes a
 * way for the second to leave the package's own directory or to name two things on two
 * filesystems. Checked before hashing rather than at unpack time, because a digest computed over a
 * traversal is a digest that attests to one.
 */
export function checkPath(path: string): string | null {
  if (path.length === 0) return 'empty'
  if (Buffer.byteLength(path, 'utf8') > 1024) return 'longer than 1024 bytes'
  if (path.startsWith('/')) return 'absolute; paths are relative to the package root'
  if (path.includes('\\')) {
    return 'contains a backslash, which is a separator on one filesystem and an ordinary filename character on another'
  }
  for (const ch of path) {
    const code = ch.codePointAt(0) ?? 0
    if (code < 0x20 || code === 0x7f) return 'contains a control character'
  }
  for (const component of path.split('/')) {
    if (component === '') return 'has an empty component, so two spellings name one file'
    if (component === '.') return 'has a `.` component'
    if (component === '..') return 'has a `..` component, which leaves the package root'
    if (component.endsWith(' ') || component.endsWith('.')) {
      return 'has a component ending in a space or a dot, which some filesystems silently strip'
    }
  }
  return null
}

/** Compare two strings by UTF-16 code unit — what JavaScript's `<` already does. */
export function utf16Compare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0
}

function scan(value: unknown, at: string): DigestError | null {
  if (typeof value === 'number') {
    const magnitude = Math.abs(value)
    const diverges =
      !Number.isFinite(value) || magnitude >= JS_MAX_PLAIN || (magnitude !== 0 && magnitude < JS_MIN_PLAIN)
    return diverges ? { kind: 'number_out_of_range', field: at === '' ? '(root)' : at, value } : null
  }
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i += 1) {
      const found = scan(value[i], `${at}[${i}]`)
      if (found) return found
    }
    return null
  }
  if (value !== null && typeof value === 'object') {
    for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
      if (item === undefined) continue
      for (const ch of key) {
        if ((ch.codePointAt(0) ?? 0) > 0xffff) {
          return { kind: 'key_outside_bmp', at: at === '' ? '(root)' : at, key }
        }
      }
      const found = scan(item, at === '' ? key : `${at}.${key}`)
      if (found) return found
    }
  }
  return null
}

/** Compute the digest of a version, or say precisely why it cannot be computed. */
export function versionDigest(params: {
  kind: string
  version: string
  manifest: unknown
  files: DigestFile[]
}): DigestOutcome {
  const bad = scan(params.manifest ?? {}, '')
  if (bad) return { ok: false, error: bad }

  for (const file of params.files) {
    const detail = checkPath(file.path)
    if (detail) return { ok: false, error: { kind: 'path', path: file.path, detail } }
    if (!isHexSha256(file.sha256)) {
      return { ok: false, error: { kind: 'sha256', path: file.path, value: file.sha256 } }
    }
  }

  const sorted = [...params.files].sort((a, b) => utf16Compare(a.path, b.path))
  for (let i = 1; i < sorted.length; i += 1) {
    if (sorted[i].path === sorted[i - 1].path) {
      return { ok: false, error: { kind: 'duplicate_path', path: sorted[i].path } }
    }
  }

  let manifestText: string
  try {
    manifestText = canonical(params.manifest ?? {})
  } catch (error) {
    return { ok: false, error: { kind: 'manifest', detail: String(error) } }
  }

  const body = [
    VERSION_DIGEST_DOMAIN,
    params.kind,
    params.version,
    manifestText,
    sorted.map(file => `${file.path}\u0000${file.sha256}`).join('\n'),
  ].join('\n')

  return { ok: true, digest: createHash('sha256').update(body, 'utf8').digest('hex') }
}
