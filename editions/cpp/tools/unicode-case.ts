/**
 * Generate the C++ edition's case tables from the engine the reference runs on.
 *
 *   node editions/cpp/tools/unicode-case.ts           # write the tables and their test vectors
 *   node editions/cpp/tools/unicode-case.ts --check   # fail if either is not what this produces
 *
 * Two files: `src/unicode_case.inc`, the tables `js.cpp` maps with, and `tests/case_vectors.inc`,
 * strings the engine converted whole, which `tests/self_check.cpp` converts with the tables and
 * compares. The tables are single code points and cannot be wrong about a string on their own; the
 * code that walks a string with them, and applies the sigma rule, can — and the vectors are what
 * would say so.
 *
 * # Why there are tables at all
 *
 * The reference upper-cases with `toUpperCase()` and lower-cases with `toLowerCase()`, and both are
 * full Unicode mappings: `ß` becomes `SS`, the `ﬁ` ligature becomes `FI`, and U+212A KELVIN SIGN
 * lower-cases to the ASCII `k`. C++ has nothing that does this. `std::toupper` is one byte, in one
 * locale, one-to-one — so the pairing-code corpus refused `ß234567` as seven characters where the
 * reference upper-cased it to the eight of a valid code. The edition therefore carries its own
 * tables, the way it carries its own `sin` and `cos` (§31.2), and for the same reason: no platform
 * library is required to agree with the reference.
 *
 * # Why they are read out of the engine rather than out of the Unicode data files
 *
 * The C++ edition is a transcription of `reference/typescript/*`, and what that code calls is
 * `toUpperCase()`. Asking the engine that runs it, one code point at a time, reproduces exactly the
 * behaviour being transcribed — including its Unicode version — where a second parser of
 * `UnicodeData.txt` and `SpecialCasing.txt` would be a second implementation of the thing, with its
 * own chances to disagree.
 *
 * # The one context rule
 *
 * Upper-casing has none. Lower-casing has exactly one outside the Turkish and Lithuanian tailorings,
 * which `toLowerCase()` does not apply: a capital sigma is final (`ς`) when a cased letter precedes
 * it and none follows, skipping case-ignorable characters in both directions. A table of single
 * code points cannot hold that, so the generator also writes the two properties the rule reads,
 * and `js.cpp` applies it.
 *
 * # What the file records about itself
 *
 * The Unicode version it was produced under. Mappings are added when Unicode adds scripts with case,
 * so a file produced under one version and checked under another is a comparison of two versions of
 * Unicode, and `--check` says that rather than reporting hundreds of code points as a defect.
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const EDITION = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const TABLES = join(EDITION, 'src', 'unicode_case.inc')
const VECTORS = join(EDITION, 'tests', 'case_vectors.inc')
const check = process.argv.includes('--check')

const LAST_CODE_POINT = 0x10ffff
/** No full case mapping in Unicode is longer than this, and `Mapping::to` is sized by it. */
const MAPPING_WIDTH = 3

type Mapping = readonly [from: number, to: readonly number[]]
type Range = readonly [first: number, last: number]

function isSurrogate(codePoint: number): boolean {
  return codePoint >= 0xd800 && codePoint <= 0xdfff
}

function codePoints(text: string): number[] {
  return Array.from(text, character => character.codePointAt(0) ?? 0)
}

function mappings(convert: (text: string) => string): Mapping[] {
  const out: Mapping[] = []
  for (let codePoint = 0; codePoint <= LAST_CODE_POINT; codePoint += 1) {
    if (isSurrogate(codePoint)) continue
    const text = String.fromCodePoint(codePoint)
    const converted = convert(text)
    if (converted === text) continue
    const to = codePoints(converted)
    if (to.length > MAPPING_WIDTH) {
      throw new Error(`U+${hex(codePoint)} maps to ${to.length} code points; Mapping::to holds ${MAPPING_WIDTH}`)
    }
    out.push([codePoint, to])
  }
  return out
}

function ranges(property: RegExp): Range[] {
  const out: [number, number][] = []
  for (let codePoint = 0; codePoint <= LAST_CODE_POINT; codePoint += 1) {
    if (isSurrogate(codePoint) || !property.test(String.fromCodePoint(codePoint))) continue
    const last = out[out.length - 1]
    if (last !== undefined && last[1] === codePoint - 1) last[1] = codePoint
    else out.push([codePoint, codePoint])
  }
  return out
}

function hex(value: number): string {
  return value.toString(16).toUpperCase().padStart(4, '0')
}

/** Rows of `items`, `perLine` to a line, each already rendered. */
function rows(items: string[], perLine: number): string {
  const lines: string[] = []
  for (let at = 0; at < items.length; at += perLine) {
    lines.push('    ' + items.slice(at, at + perLine).join(', ') + ',')
  }
  return lines.join('\n')
}

function mappingTable(name: string, table: Mapping[]): string {
  const rendered = table.map(([from, to]) => {
    const slots = [...to, ...Array<number>(MAPPING_WIDTH - to.length).fill(0)]
    return `{0x${hex(from)}, {${slots.map(slot => `0x${hex(slot)}`).join(', ')}}}`
  })
  return `constexpr Mapping ${name}[] = {\n${rows(rendered, 3)}\n};`
}

function rangeTable(name: string, table: Range[]): string {
  const rendered = table.map(([first, last]) => `{0x${hex(first)}, 0x${hex(last)}}`)
  return `constexpr Range ${name}[] = {\n${rows(rendered, 5)}\n};`
}

const unicode = process.versions.unicode
if (unicode === undefined) {
  console.error('This Node reports no Unicode version, so the tables it would produce could not say which they are.')
  process.exit(1)
}

const CASED = /^\p{Cased}$/u
const CASE_IGNORABLE = /^\p{Case_Ignorable}$/u

const upper = mappings(text => text.toUpperCase())
const lower = mappings(text => text.toLowerCase())
const cased = ranges(CASED)
const ignorable = ranges(CASE_IGNORABLE)

const tables = `// Generated by editions/cpp/tools/unicode-case.ts. Do not edit; run the generator.
//
// Unicode ${unicode}: \`String.prototype.toUpperCase\` and \`toLowerCase\`, one code point at a time,
// for every code point whose mapping is not itself; and the two properties \`toLowerCase\`'s one
// context rule reads. Included by src/js.cpp inside its unnamed namespace, which declares
// \`Mapping\` and \`Range\` before including this.
//
// ${upper.length} upper-case mappings, ${lower.length} lower-case, ${cased.length} Cased ranges, ${ignorable.length} Case_Ignorable ranges.

constexpr std::string_view kUnicodeVersion = "${unicode}";

${mappingTable('kUpper', upper)}

${mappingTable('kLower', lower)}

${rangeTable('kCased', cased)}

${rangeTable('kCaseIgnorable', ignorable)}
`

// ── The vectors ─────────────────────────────────────────────────────────────

const CAPITAL_SIGMA = 0x03a3

/**
 * Two characters from each of the four classes the sigma rule tells apart — cased only,
 * case-ignorable only, both, and neither — the first of each class in the code space and the first
 * at or above the Greek block, so Greek letters take part.
 *
 * "Both" is the class these exist for. A modifier letter is cased **and** case-ignorable, and
 * whether it counts as the letter before a sigma or is stepped over is the engine's decision, read
 * here rather than guessed in `js.cpp`.
 */
function representatives(): number[] {
  const classes = new Map<string, number[]>()
  for (let codePoint = 0x21; codePoint <= LAST_CODE_POINT; codePoint += 1) {
    if (isSurrogate(codePoint) || codePoint === CAPITAL_SIGMA) continue
    const text = String.fromCodePoint(codePoint)
    const key = `${CASED.test(text)}/${CASE_IGNORABLE.test(text)}`
    const found = classes.get(key) ?? []
    const wanted = found.length === 0 || (found.length === 1 && codePoint >= 0x0370 && found[0] < 0x0370)
    if (wanted) classes.set(key, [...found, codePoint])
  }
  const picked = [...classes.values()].flat()
  if (classes.size !== 4 || picked.length !== 8) {
    throw new Error(`expected two characters in each of four classes, found ${JSON.stringify([...classes])}`)
  }
  return picked.sort((a, b) => a - b)
}

type Vector = readonly [upper: boolean, input: readonly number[]]

function vectors(): Vector[] {
  const reps = representatives()
  const out: Vector[] = [[false, [CAPITAL_SIGMA]]]
  for (const x of reps) {
    out.push([false, [x, CAPITAL_SIGMA]], [false, [CAPITAL_SIGMA, x]])
    for (const y of reps) {
      out.push([false, [x, CAPITAL_SIGMA, y]], [false, [x, y, CAPITAL_SIGMA]], [false, [CAPITAL_SIGMA, x, y]])
    }
    out.push([false, [x, CAPITAL_SIGMA, CAPITAL_SIGMA, x]])
  }
  // Every multi-character mapping, walked as one string each way, so the code that appends a
  // mapping's code points is exercised on all of them rather than on the two a corpus case happens
  // to hold.
  out.push([true, upper.filter(([, to]) => to.length > 1).map(([from]) => from)])
  out.push([false, lower.filter(([, to]) => to.length > 1).map(([from]) => from)])
  return out
}

function u32(points: readonly number[]): string {
  return 'U"' + points.map(point => `\\U${point.toString(16).toUpperCase().padStart(8, '0')}`).join('') + '"'
}

const cases = vectors()
const rendered = cases.map(([isUpper, input]) => {
  const text = String.fromCodePoint(...input)
  const expected = codePoints(isUpper ? text.toUpperCase() : text.toLowerCase())
  return `    {${isUpper}, ${u32(input)}, ${u32(expected)}},`
})

const vectorFile = `// Generated by editions/cpp/tools/unicode-case.ts. Do not edit; run the generator.
//
// Unicode ${unicode}: whole strings converted by the engine the reference runs on, for
// tests/self_check.cpp to convert with src/unicode_case.inc and compare. ${cases.length} vectors.
// \`upper\` is \`toUpperCase\`; otherwise \`toLowerCase\`.

constexpr std::string_view kVectorsUnicodeVersion = "${unicode}";

constexpr CaseVector kCaseVectors[] = {
${rendered.join('\n')}
};
`

function drifted(path: string, label: string, expected: string): boolean {
  let held: string
  try {
    held = readFileSync(path, 'utf8')
  } catch {
    console.error(`${label} does not exist. Run this without --check.`)
    return true
  }
  if (held === expected) return false
  const heldVersion = /^constexpr std::string_view k\w*UnicodeVersion = "([^"]*)";$/m.exec(held)?.[1]
  if (heldVersion !== undefined && heldVersion !== unicode) {
    console.error(
      `${label} was generated under Unicode ${heldVersion} and this Node is Unicode ${unicode}. The `
        + 'difference is the case pairs Unicode added between the two, not a defect in either. Run the '
        + 'check on a Node carrying the same Unicode version, or regenerate and review the diff as '
        + 'the change to the edition it is.',
    )
  } else {
    console.error(
      `${label} is not what this generator produces. It was edited by hand, or the generator changed `
        + 'without being rerun. Run `node editions/cpp/tools/unicode-case.ts`.',
    )
  }
  return true
}

if (check) {
  const tablesDrifted = drifted(TABLES, 'src/unicode_case.inc', tables)
  const vectorsDrifted = drifted(VECTORS, 'tests/case_vectors.inc', vectorFile)
  if (tablesDrifted || vectorsDrifted) process.exit(1)
  console.log(`unicode_case.inc and case_vectors.inc match: Unicode ${unicode}, ${cases.length} vectors`)
} else {
  writeFileSync(TABLES, tables)
  writeFileSync(VECTORS, vectorFile)
  console.log(
    `wrote src/unicode_case.inc — Unicode ${unicode}: ${upper.length} upper, ${lower.length} lower, `
      + `${cased.length} Cased and ${ignorable.length} Case_Ignorable ranges — and ${cases.length} `
      + 'vectors to tests/case_vectors.inc',
  )
}
