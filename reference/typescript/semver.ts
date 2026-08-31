/**
 * Minimal semver parsing, comparison and range satisfaction.
 *
 * Skynet versions everything — models, datasets, skills, environments, benchmarks, adapters
 * and the runtime — so ordering and range checks are core platform logic, not a UI detail.
 * A dependency would work too, but this is small, dependency-free and exactly the subset the
 * platform uses (no build metadata, no complex range unions).
 *
 * ── Why this file is in the reference ───────────────────────────────────────
 *
 * It arrived here as a consequence of §54.2. Preconditions become predicates the OS evaluates
 * before a skill may start, `semver_satisfies` is one of the operators, and a robot deciding
 * whether its runtime satisfies `^1.2.0` differently from the Hub that sold it the skill is the
 * §31.2 failure: *a format with two implementations of its own definition is not a format.* So
 * range satisfaction is a contract, and this is where contracts live.
 *
 * Note what is **not** unified. `package.ts` compares OS versions with its own numeric-segment
 * comparator, deliberately: `min_os_version` is matched against whatever an image reports, which
 * is `2.0` as often as `2.0.0`, and `compareVersions` here returns 0 for anything it cannot parse.
 * A strict parser in that position would silently admit a package onto an older OS. Two
 * comparators with two jobs, and this comment is why neither is the other's duplicate.
 */

export interface ParsedVersion {
  major: number
  minor: number
  patch: number
  prerelease: string | null
  raw: string
}

const SEMVER_RE = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/

export function parseVersion(input: string): ParsedVersion | null {
  const match = SEMVER_RE.exec(input.trim())
  if (!match) return null
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ?? null,
    raw: input.trim(),
  }
}

export function isValidVersion(input: string): boolean {
  return parseVersion(input) !== null
}

/** Compare prerelease identifiers per semver §11: numeric < alphanumeric, longer wins ties. */
function comparePrerelease(a: string | null, b: string | null): number {
  if (a === b) return 0
  // A version without a prerelease outranks one with it.
  if (a === null) return 1
  if (b === null) return -1

  const aParts = a.split('.')
  const bParts = b.split('.')
  const length = Math.max(aParts.length, bParts.length)

  for (let i = 0; i < length; i++) {
    const ai = aParts[i]
    const bi = bParts[i]
    if (ai === undefined) return -1
    if (bi === undefined) return 1
    const aNum = /^\d+$/.test(ai)
    const bNum = /^\d+$/.test(bi)
    if (aNum && bNum) {
      const diff = Number(ai) - Number(bi)
      if (diff !== 0) return diff < 0 ? -1 : 1
      continue
    }
    if (aNum) return -1
    if (bNum) return 1
    if (ai !== bi) return ai < bi ? -1 : 1
  }
  return 0
}

/** -1 if a < b, 0 if equal, 1 if a > b. */
export function compareVersions(a: string | ParsedVersion, b: string | ParsedVersion): number {
  const pa = typeof a === 'string' ? parseVersion(a) : a
  const pb = typeof b === 'string' ? parseVersion(b) : b
  if (!pa || !pb) return 0
  if (pa.major !== pb.major) return pa.major < pb.major ? -1 : 1
  if (pa.minor !== pb.minor) return pa.minor < pb.minor ? -1 : 1
  if (pa.patch !== pb.patch) return pa.patch < pb.patch ? -1 : 1
  return comparePrerelease(pa.prerelease, pb.prerelease)
}

/** Newest first. Used everywhere a version list is rendered. */
export function sortVersionsDesc<T extends { version: string }>(versions: T[]): T[] {
  return [...versions].sort((a, b) => compareVersions(b.version, a.version))
}

export function nextVersion(current: string, bump: 'major' | 'minor' | 'patch'): string {
  const parsed = parseVersion(current) ?? { major: 0, minor: 0, patch: 0, prerelease: null, raw: current }
  if (bump === 'major') return `${parsed.major + 1}.0.0`
  if (bump === 'minor') return `${parsed.major}.${parsed.minor + 1}.0`
  return `${parsed.major}.${parsed.minor}.${parsed.patch + 1}`
}

type Comparator = { op: '>=' | '>' | '<=' | '<' | '=' | '^' | '~'; version: ParsedVersion }

function parseComparator(token: string): Comparator | null {
  const match = /^(>=|<=|>|<|\^|~|=)?\s*(.+)$/.exec(token.trim())
  if (!match) return null
  const version = parseVersion(match[2])
  if (!version) return null
  return { op: (match[1] as Comparator['op']) ?? '=', version }
}

function satisfiesComparator(target: ParsedVersion, comparator: Comparator): boolean {
  const cmp = compareVersions(target, comparator.version)
  switch (comparator.op) {
    case '>=': return cmp >= 0
    case '>':  return cmp > 0
    case '<=': return cmp <= 0
    case '<':  return cmp < 0
    case '=':  return cmp === 0
    case '^': {
      // Caret: compatible within the leftmost non-zero component.
      if (cmp < 0) return false
      const base = comparator.version
      if (base.major > 0) return target.major === base.major
      if (base.minor > 0) return target.major === 0 && target.minor === base.minor
      return target.major === 0 && target.minor === 0 && target.patch === base.patch
    }
    case '~': {
      // Tilde: patch-level changes within the same minor.
      if (cmp < 0) return false
      return target.major === comparator.version.major && target.minor === comparator.version.minor
    }
    default: return false
  }
}

/**
 * A range, parsed into its `||` groups.
 *
 * A group that does not parse is `null` rather than an error for the whole range, which keeps
 * `satisfies` exactly as it behaved before this function was factored out of it: one unreadable
 * alternative does not invalidate the readable ones. `isValidRange` is the caller that wants the
 * stricter answer, and it asks for it by checking every group rather than by having its own parser.
 *
 * `null` for the whole result means the range is one of the wildcards — every version satisfies it
 * and there is nothing to compare against.
 */
function parseRange(range: string): (Comparator[] | null)[] | null {
  const trimmed = range.trim()
  if (!trimmed || trimmed === '*' || trimmed.toLowerCase() === 'any') return null

  return trimmed.split('||').map(group => {
    // Tokenize on the operator rather than on whitespace so ">= 1.2.0" and ">=1.2.0" both parse.
    const tokens = group.trim().match(/(?:>=|<=|>|<|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?/g)
    if (!tokens || tokens.length === 0) return null

    const comparators = tokens.map(parseComparator)
    if (comparators.some(c => c === null)) return null
    return comparators as Comparator[]
  })
}

/**
 * Does `version` satisfy `range`?
 *
 * Supports `*`, space-separated AND comparators (`>=1.2.0 <2.0.0`) and `||` OR groups.
 * Adapters express their supported runtime window this way, and the compatibility engine
 * exposes it as the `semver_satisfies` predicate.
 */
export function satisfies(version: string, range: string): boolean {
  const target = parseVersion(version)
  if (!target) return false

  const groups = parseRange(range)
  if (groups === null) return true

  return groups.some(group => group !== null && group.every(c => satisfiesComparator(target, c)))
}

/**
 * Is `range` a range this edition can read at all?
 *
 * `satisfies` answers false both for "this version is outside that window" and for "that is not a
 * window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is a
 * skill nobody can ever run here, and its publisher is the one who has to hear about it. A battery
 * that is too low is a different sentence with a different remedy.
 */
export function isValidRange(range: string): boolean {
  const groups = parseRange(range)
  return groups === null || groups.every(group => group !== null)
}
