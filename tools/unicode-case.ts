/**
 * Generate the editions' case tables from the engine the reference runs on.
 *
 *   node tools/unicode-case.ts           # write every target's tables and test vectors
 *   node tools/unicode-case.ts --check   # fail if any of them is not what this produces
 *
 * Two artefacts per target: the tables an edition maps with, and strings the engine converted
 * whole, which that edition's self-check converts with the tables and compares. The tables are
 * single code points and cannot be wrong about a string on their own; the code that walks a string
 * with them, and applies the sigma rule, can — and the vectors are what would say so (finding 163).
 *
 * # Why there are tables at all
 *
 * The reference upper-cases with `toUpperCase()` and lower-cases with `toLowerCase()`, and both are
 * full Unicode mappings: `ß` becomes `SS`, the `ﬁ` ligature becomes `FI`, and U+212A KELVIN SIGN
 * lower-cases to the ASCII `k`. C++ has nothing that does this. `std::toupper` is one byte, in one
 * locale, one-to-one — so the pairing-code corpus refused `ß234567` as seven characters where the
 * reference upper-cased it to the eight of a valid code. Go has `strings.ToUpper`, which is not the
 * same function either: it is the *simple* mapping, so it leaves `ß` and `ﬁ` alone, and
 * `strings.ToLower` has no final-sigma rule. Each edition therefore carries its own tables, the way
 * it carries its own `sin` and `cos` (§31.2), and for the same reason: no platform library is
 * required to agree with the reference.
 *
 * # Why they are read out of the engine rather than out of the Unicode data files
 *
 * An edition is a transcription of `reference/typescript/*`, and what that code calls is
 * `toUpperCase()`. Asking the engine that runs it, one code point at a time, reproduces exactly the
 * behaviour being transcribed — including its Unicode version — where a second parser of
 * `UnicodeData.txt` and `SpecialCasing.txt` would be a second implementation of the thing, with its
 * own chances to disagree.
 *
 * # Why one generator rather than one per edition
 *
 * Three targets today — C, C++ and Go — and each only says how to *render* the one derivation.\n *\n * This file lived at `editions/cpp/tools/unicode-case.ts` while there was one edition that needed
 * it. Copying it beside the second would have put the derivation — which code points map, which
 * characters the sigma rule reads, which eight representatives the vectors are built from — in two
 * places, and §3.3's rule is that every fact exists in exactly one place and everything else
 * derives from it. Two copies of this would agree on the day they were written and diverge at the
 * next Unicode version, in whichever edition was regenerated second. So the derivation is written
 * once and each target only says how to *render* it.
 *
 * # The one context rule
 *
 * Upper-casing has none. Lower-casing has exactly one outside the Turkish and Lithuanian tailorings,
 * which `toLowerCase()` does not apply: a capital sigma is final (`ς`) when a cased letter precedes
 * it and none follows, skipping case-ignorable characters in both directions. A table of single
 * code points cannot hold that, so the generator also writes the two properties the rule reads, and
 * each edition applies it.
 *
 * # What the files record about themselves
 *
 * The Unicode version they were produced under. Mappings are added when Unicode adds scripts with
 * case, so a file produced under one version and checked under another is a comparison of two
 * versions of Unicode, and `--check` says that rather than reporting hundreds of code points as a
 * defect.
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const GENERATOR = 'tools/unicode-case.ts'
const check = process.argv.includes('--check')

const LAST_CODE_POINT = 0x10ffff
/** No full case mapping in Unicode is longer than this, and the C++ `Mapping::to` is sized by it. */
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
function rows(items: string[], perLine: number, indent = '    '): string {
  const lines: string[] = []
  for (let at = 0; at < items.length; at += perLine) {
    lines.push(indent + items.slice(at, at + perLine).join(', ') + ',')
  }
  return lines.join('\n')
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

// ── The vectors ─────────────────────────────────────────────────────────────

const CAPITAL_SIGMA = 0x03a3

/**
 * Two characters from each of the four classes the sigma rule tells apart — cased only,
 * case-ignorable only, both, and neither — the first of each class in the code space and the first
 * at or above the Greek block, so Greek letters take part.
 *
 * "Both" is the class these exist for. A modifier letter is cased **and** case-ignorable, and
 * whether it counts as the letter before a sigma or is stepped over is the engine's decision, read
 * here rather than guessed in an edition.
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

const cases = vectors()

/** A vector's expectation, computed by the engine from the whole string. */
function expectation([isUpper, input]: Vector): number[] {
  const text = String.fromCodePoint(...input)
  return codePoints(isUpper ? text.toUpperCase() : text.toLowerCase())
}

// ── The C++ target ──────────────────────────────────────────────────────────

function cppMappingTable(name: string, table: Mapping[]): string {
  const rendered = table.map(([from, to]) => {
    const slots = [...to, ...Array<number>(MAPPING_WIDTH - to.length).fill(0)]
    return `{0x${hex(from)}, {${slots.map(slot => `0x${hex(slot)}`).join(', ')}}}`
  })
  return `constexpr Mapping ${name}[] = {\n${rows(rendered, 3)}\n};`
}

function cppRangeTable(name: string, table: Range[]): string {
  const rendered = table.map(([first, last]) => `{0x${hex(first)}, 0x${hex(last)}}`)
  return `constexpr Range ${name}[] = {\n${rows(rendered, 5)}\n};`
}

function cppU32(points: readonly number[]): string {
  return 'U"' + points.map(point => `\\U${point.toString(16).toUpperCase().padStart(8, '0')}`).join('') + '"'
}

const cppTables = `// Generated by ${GENERATOR}. Do not edit; run the generator.
//
// Unicode ${unicode}: \`String.prototype.toUpperCase\` and \`toLowerCase\`, one code point at a time,
// for every code point whose mapping is not itself; and the two properties \`toLowerCase\`'s one
// context rule reads. Included by src/js.cpp inside its unnamed namespace, which declares
// \`Mapping\` and \`Range\` before including this.
//
// ${upper.length} upper-case mappings, ${lower.length} lower-case, ${cased.length} Cased ranges, ${ignorable.length} Case_Ignorable ranges.

constexpr std::string_view kUnicodeVersion = "${unicode}";

${cppMappingTable('kUpper', upper)}

${cppMappingTable('kLower', lower)}

${cppRangeTable('kCased', cased)}

${cppRangeTable('kCaseIgnorable', ignorable)}
`

const cppVectors = `// Generated by ${GENERATOR}. Do not edit; run the generator.
//
// Unicode ${unicode}: whole strings converted by the engine the reference runs on, for
// tests/self_check.cpp to convert with src/unicode_case.inc and compare. ${cases.length} vectors.
// \`upper\` is \`toUpperCase\`; otherwise \`toLowerCase\`.

constexpr std::string_view kVectorsUnicodeVersion = "${unicode}";

constexpr CaseVector kCaseVectors[] = {
${cases.map(vector => `    {${vector[0]}, ${cppU32(vector[1])}, ${cppU32(expectation(vector))}},`).join('\n')}
};
`

// ── The Go target ───────────────────────────────────────────────────────────
//
// Two sorted slices per mapping rather than a map: a Go map literal of this size is slow to compile
// and its iteration order is randomised, and while nothing here iterates one today, a table nobody
// may walk in order is a table the next reader cannot print. Sorted slices binary-search in the
// same time and render as a diffable file.

/** A Go interpreted string literal holding exactly these code points, ASCII-only in the source. */
function goString(points: readonly number[]): string {
  return '"' + points.map(point =>
    point <= 0xffff
      ? `\\u${point.toString(16).toUpperCase().padStart(4, '0')}`
      : `\\U${point.toString(16).toUpperCase().padStart(8, '0')}`,
  ).join('') + '"'
}

function goMappingTable(name: string, table: Mapping[]): string {
  const from = rows(table.map(([codePoint]) => `0x${hex(codePoint)}`), 8, '\t')
  const to = rows(table.map(([, points]) => goString(points)), 4, '\t')
  return `var ${name}From = []rune{\n${from}\n}\n\nvar ${name}To = []string{\n${to}\n}`
}

function goRangeTable(name: string, table: Range[]): string {
  const rendered = table.map(([first, last]) => `{0x${hex(first)}, 0x${hex(last)}}`)
  return `var ${name} = []runeRange{\n${rows(rendered, 4, '\t')}\n}`
}

const goTables = `// Generated by ${GENERATOR}. Do not edit; run the generator.
//
// Unicode ${unicode}: String.prototype.toUpperCase and toLowerCase, one code point at a time, for
// every code point whose mapping is not itself; and the two properties toLowerCase's one context
// rule reads. Read by ToUpper and ToLower in case.go, which declares runeRange.
//
// ${upper.length} upper-case mappings, ${lower.length} lower-case, ${cased.length} Cased ranges, ${ignorable.length} Case_Ignorable ranges.
//
// The From slices are ascending, which ToUpper and ToLower binary-search and TestCaseTablesSorted
// checks, because a table that lost its order would still answer most lookups.

package js

// unicodeVersion is the Unicode version of the engine these tables were read out of. Compared with
// the vectors' in the self-check: two files generated under different versions would disagree about
// characters neither of them is wrong about.
const unicodeVersion = ${JSON.stringify(unicode)}

${goMappingTable('upper', upper)}

${goMappingTable('lower', lower)}

${goRangeTable('casedRanges', cased)}

${goRangeTable('caseIgnorableRanges', ignorable)}
`

const goVectors = `// Generated by ${GENERATOR}. Do not edit; run the generator.
//
// Unicode ${unicode}: whole strings converted by the engine the reference runs on, for case_test.go
// to convert with the tables in unicode_case.go and compare. ${cases.length} vectors.
// Upper is toUpperCase; otherwise toLowerCase.

package js

type caseVector struct {
	Upper bool
	In    string
	Out   string
}

const vectorsUnicodeVersion = ${JSON.stringify(unicode)}

var caseVectors = []caseVector{
${cases.map(vector => `\t{${vector[0]}, ${goString(vector[1])}, ${goString(expectation(vector))}},`).join('\n')}
}
`

// ── The C target ────────────────────────────────────────────────────────────
//
// Two parallel arrays per mapping, like the Go target and for the same reason a C reader needs even
// more: `from` is ascending so `sk_js_upper` can binary-search it, and a table that lost its order
// would still answer most lookups. `to` is fixed-width because C has no variable-length aggregate in
// a static initialiser, and the width is MAPPING_WIDTH — the generator throws above it rather than
// silently truncating a mapping.

/** A C `uint32_t` literal for one code point. */
function cHex(value: number): string {
  return `0x${hex(value)}`
}

function cMappingTable(name: string, table: Mapping[]): string {
  const from = rows(table.map(([codePoint]) => cHex(codePoint)), 8)
  const to = rows(
    table.map(([, points]) => {
      const slots = [...points, ...Array<number>(MAPPING_WIDTH - points.length).fill(0)]
      return `{${slots.map(cHex).join(', ')}}`
    }),
    3,
  )
  return (
    `static const uint32_t ${name}_from[] = {\n${from}\n};\n\n`
    + `static const uint32_t ${name}_to[][SK_CASE_WIDTH] = {\n${to}\n};`
  )
}

function cRangeTable(name: string, table: Range[]): string {
  const rendered = table.map(([first, last]) => `{${cHex(first)}, ${cHex(last)}}`)
  return `static const sk_code_range ${name}[] = {\n${rows(rendered, 4)}\n};`
}

/**
 * A C `char32_t` string literal holding exactly these code points, ASCII-only in the source.
 *
 * Hex escapes rather than `\\U` universal character names, which is where C and C++ part company: C
 * forbids a universal character name for any code point below 0x00A0 apart from `$`, `@` and the
 * backtick, so the C++ target's rendering does not compile as C at all — every vector carrying a
 * Latin letter is rejected. A hex escape has no such rule, and greedy hex-digit consumption is not a
 * hazard here because every element of the literal begins with a backslash.
 */
function cU32(points: readonly number[]): string {
  if (points.length === 0) return 'U""'
  return 'U"' + points.map(point => `\\x${point.toString(16).toUpperCase()}`).join('') + '"'
}

const cTables = `/* Generated by ${GENERATOR}. Do not edit; run the generator.
 *
 * Unicode ${unicode}: \`String.prototype.toUpperCase\` and \`toLowerCase\`, one code point at a time, for
 * every code point whose mapping is not itself; and the two properties \`toLowerCase\`'s one context
 * rule reads. Included by src/js.c, which declares SK_CASE_WIDTH and sk_code_range before including
 * this.
 *
 * ${upper.length} upper-case mappings, ${lower.length} lower-case, ${cased.length} Cased ranges, ${ignorable.length} Case_Ignorable ranges.
 *
 * The \`_from\` arrays are ascending, which the lookups binary-search and the self-check asserts.
 */

#define SK_UNICODE_VERSION "${unicode}"

${cMappingTable('upper', upper)}

${cMappingTable('lower', lower)}

${cRangeTable('cased_ranges', cased)}

${cRangeTable('case_ignorable_ranges', ignorable)}
`

const cVectors = `/* Generated by ${GENERATOR}. Do not edit; run the generator.
 *
 * Unicode ${unicode}: whole strings converted by the engine the reference runs on, for
 * src/selfcheck.c to convert with the tables in src/unicode_case.inc and compare. ${cases.length} vectors.
 * \`upper\` is \`toUpperCase\`; otherwise \`toLowerCase\`.
 *
 * The tables are single code points and cannot be wrong about a string on their own. The code that
 * walks a string with them, and applies the sigma rule, can — and these are what would say so
 * (finding 163).
 */

#define SK_VECTORS_UNICODE_VERSION "${unicode}"

static const sk_case_vector case_vectors[] = {
${cases.map(vector => `    {${vector[0] ? 'true' : 'false'}, ${cU32(vector[1])}, ${cU32(expectation(vector))}},`).join('\n')}
};
`

// ── Writing them out ────────────────────────────────────────────────────────

type Artefact = { path: string; content: string }

const artefacts: Artefact[] = [
  { path: join(ROOT, 'editions', 'cpp', 'src', 'unicode_case.inc'), content: cppTables },
  { path: join(ROOT, 'editions', 'cpp', 'tests', 'case_vectors.inc'), content: cppVectors },
  { path: join(ROOT, 'editions', 'go', 'internal', 'js', 'unicode_case.go'), content: goTables },
  { path: join(ROOT, 'editions', 'go', 'internal', 'js', 'case_vectors_test.go'), content: goVectors },
  { path: join(ROOT, 'editions', 'c', 'src', 'unicode_case.inc'), content: cTables },
  { path: join(ROOT, 'editions', 'c', 'tests', 'case_vectors.inc'), content: cVectors },
]

function label(path: string): string {
  return relative(ROOT, path).replace(/\\/g, '/')
}

function drifted({ path, content }: Artefact): boolean {
  const name = label(path)
  let held: string
  try {
    held = readFileSync(path, 'utf8')
  } catch {
    console.error(`${name} does not exist. Run this without --check.`)
    return true
  }
  if (held === content) return false
  const heldVersion = /Unicode(?:Version)?\s*=\s*"([0-9.]+)"/.exec(held)?.[1]
  if (heldVersion !== undefined && heldVersion !== unicode) {
    console.error(
      `${name} was generated under Unicode ${heldVersion} and this Node is Unicode ${unicode}. The `
        + 'difference is the case pairs Unicode added between the two, not a defect in either. Run the '
        + 'check on a Node carrying the same Unicode version, or regenerate and review the diff as '
        + 'the change to the editions it is.',
    )
  } else {
    console.error(
      `${name} is not what this generator produces. It was edited by hand, or the generator changed `
        + `without being rerun. Run \`node ${GENERATOR}\`.`,
    )
  }
  return true
}

if (check) {
  const bad = artefacts.filter(drifted)
  if (bad.length > 0) process.exit(1)
  console.log(`${artefacts.length} generated files match: Unicode ${unicode}, ${cases.length} vectors`)
} else {
  for (const artefact of artefacts) writeFileSync(artefact.path, artefact.content)
  console.log(
    `Unicode ${unicode}: ${upper.length} upper, ${lower.length} lower, ${cased.length} Cased and `
      + `${ignorable.length} Case_Ignorable ranges, ${cases.length} vectors\n`
      + artefacts.map(artefact => `  wrote ${label(artefact.path)}`).join('\n'),
  )
}
