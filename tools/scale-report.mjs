#!/usr/bin/env node
/**
 * Measure how large this system actually is, and hold §73's summary to it.
 *
 *   node tools/scale-report.mjs                    # regenerate reference/reports/scale.json
 *   node tools/scale-report.mjs --check            # fail if the file, or §73, has drifted
 *   node tools/scale-report.mjs "../Robot Overflow" # include the Hub's mirror in the measurement
 *
 * # Why this file exists
 *
 * §73 opens with an amendment explaining that its own table had gone stale "in the ordinary way —
 * nobody updates a summary while they are building the thing it summarises" — and that a state
 * table which understates the platform is worse than no table, because it is the first thing a
 * vendor reads. It then closes with a paragraph of typed figures.
 *
 * On 2026-09-05 that paragraph said eighteen crates when there were twenty, one edition when there
 * were two, and 1,055 corpus cases when there were 1,116. The document had diagnosed the disease
 * and caught it again in the same section, which is about as clear a signal as a repository gives
 * that a number in prose needs a source.
 *
 * So the figures are measured here and pinned in `reference/reports/scale.json`, and `--check`
 * verifies two separate things:
 *
 * 1. the pinned file is what a run produces — the same contract `edition-report.mjs --check` has;
 * 2. every figure §73's closing paragraph states **still matches the measurement**, at the
 *    precision the paragraph states it to.
 *
 * # On precision, which is the whole design of the check
 *
 * §73 says *"numbers go stale; the shape does not"*, and a check that failed whenever somebody
 * added a test would be a check people turn off. So the two kinds of figure are treated
 * differently, and the difference is in the prose itself:
 *
 * * **Rounded figures** — "106,000 lines of Rust" — are matched after rounding the measurement the
 *   same way. Adding a hundred lines does not fail; adding eight thousand does.
 * * **Exact figures** — crate count, corpus files, corpus cases, edition count — are matched
 *   exactly, because each is a number somebody could recount in a second and each one was wrong.
 *
 * Test counts are deliberately *not* in the checked set. They change on every commit that adds a
 * test, they are reported by the suites themselves on every run, and a document is not where
 * anybody should read them.
 *
 * # What it does not measure
 *
 * The Hub's own test suite. That number belongs to the other repository and this one cannot run it;
 * §73 states it as what it is. The Hub's *mirror* of the shared contracts is measured when a
 * checkout is given or found beside this one, and carried from the pin when it is not — the same
 * shape as `check-contract-mirrors.mjs`, and for the same reason: a vendor who cloned only this
 * repository is never told their tree is broken because of a directory they were never given.
 */


import { readdirSync, readFileSync, statSync, writeFileSync, existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const PIN = join(ROOT, 'reference', 'reports', 'scale.json')
const SPEC = join(ROOT, 'docs', 'skynet-os-architecture.md')

let hubMeasured = false

const args = process.argv.slice(2)
const check = args.includes('--check')
const hubArg = args.find((a) => !a.startsWith('--'))

/** Every file under `dir` whose name ends in one of `extensions`, skipping the noise. */
function walk(dir, extensions, skip = []) {
  const out = []
  if (!existsSync(dir)) return out
  for (const entry of readdirSync(dir)) {
    if (entry === 'node_modules' || entry === '__pycache__' || entry === 'target'
      || entry === '.git' || skip.includes(entry)) continue
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) out.push(...walk(path, extensions, skip))
    else if (extensions.some((e) => entry.endsWith(e))) out.push(path)
  }
  return out
}

function lines(files) {
  let total = 0
  for (const file of files) {
    const text = readFileSync(file, 'utf8')
    total += text.length === 0 ? 0 : text.split('\n').length - (text.endsWith('\n') ? 1 : 0)
  }
  return total
}

function measureCorpus() {
  const files = walk(join(ROOT, 'corpus'), ['.json'])
  let cases = 0
  for (const file of files) cases += (JSON.parse(readFileSync(file, 'utf8')).cases ?? []).length
  return { files: files.length, cases }
}

function measureHubMirror() {
  const candidates = [
    hubArg && resolve(hubArg),
    resolve(ROOT, '..', 'Robot Overflow'),
    resolve(ROOT, '..', 'BotMarket'),
  ].filter(Boolean)
  for (const base of candidates) {
    const mirror = join(base, 'lib', 'skynet', 'os')
    if (existsSync(mirror)) {
      return { lines: lines(walk(mirror, ['.ts', '.tsx'])), measured: true, from: base }
    }
  }
  return null
}

function measure() {
  const rustFiles = walk(join(ROOT, 'crates'), ['.rs'])
  const crates = readdirSync(join(ROOT, 'crates'))
    .filter((e) => statSync(join(ROOT, 'crates', e)).isDirectory())

  const editions = JSON.parse(readFileSync(
    join(ROOT, 'reference', 'reports', 'editions.json'), 'utf8'))

  const previous = existsSync(PIN) ? JSON.parse(readFileSync(PIN, 'utf8')) : null
  const hub = measureHubMirror()

  // Whether *this run* could see the Hub is a fact about the machine it ran on, and it must not
  // reach the pinned file — CI has one checkout, so a `measured: false` written there would make
  // the pin disagree with every run that had both trees and vice versa. That is a check which
  // fails for a reason with nothing to do with what it guards, which is worse than no check: it
  // trains people to ignore it. The file carries the number; the console says where it came from.
  hubMeasured = hub !== null

  return {
    note: 'Measured by tools/scale-report.mjs. §73 quotes these; --check holds it to them. '
      + 'Never edit by hand — a figure somebody corrected to match a stale document is a check '
      + 'that has been switched off.',
    rust: { crates: crates.length, files: rustFiles.length, lines: lines(rustFiles) },
    reference_typescript: {
      lines: lines(walk(join(ROOT, 'reference', 'typescript'), ['.ts'])),
    },
    editions: {
      count: editions.editions.length,
      ids: editions.editions.map((e) => e.id),
      python_lines: lines(walk(join(ROOT, 'editions', 'python'), ['.py'])),
    },
    drivers: {
      lines: lines(walk(join(ROOT, 'drivers'), ['.py', '.c', '.cpp', '.h', '.hpp', '.rs'])),
    },
    corpus: measureCorpus(),
    hub_mirror: { lines: hub ? hub.lines : previous?.hub_mirror?.lines ?? null },
  }
}

/** Round to the nearest thousand, which is the precision §73 states line counts to. */
const thousands = (n) => Math.round(n / 1000) * 1000

/**
 * The claims §73's closing paragraph makes, and how to check each one.
 *
 * The patterns read the document rather than a copy of it, so a figure edited in one place and not
 * the other is a failure rather than a silence. Each entry says what it expects and at what
 * precision — `exact` for a number somebody could recount, `thousands` for one written rounded.
 */
function claims(m) {
  return [
    {
      label: 'lines of Rust',
      pattern: /\*\*([\d,]+) lines of Rust\*\*/,
      expected: thousands(m.rust.lines),
      precision: 'thousands',
    },
    {
      label: 'crates',
      // `[\w-]` rather than `\w`: the count went from "twenty" to "twenty-one" and the
      // pattern stopped matching, which this check reported as the figure being absent. A number
      // word with a hyphen in it is still a number word.
      pattern: /across \*\*([\w-]+) crates\*\*/,
      expected: m.rust.crates,
      precision: 'exact',
      words: true,
    },
    {
      label: 'corpus files',
      pattern: /a corpus of (\d+) files/,
      expected: m.corpus.files,
      precision: 'exact',
    },
    {
      label: 'corpus cases',
      pattern: /files and ([\d,]+) cases/,
      expected: m.corpus.cases,
      precision: 'exact',
    },
    {
      label: 'lines of the TypeScript reference',
      pattern: /\*\*([\d,]+) lines\*\* of the TypeScript reference/,
      expected: thousands(m.reference_typescript.lines),
      precision: 'thousands',
    },
    {
      label: 'lines of the Python edition',
      pattern: /\*\*([\d,]+) lines\*\* of the Python edition/,
      expected: thousands(m.editions.python_lines),
      precision: 'thousands',
    },
    {
      label: 'lines of drivers',
      pattern: /\*\*([\d,]+) lines\*\* of drivers/,
      expected: thousands(m.drivers.lines),
      precision: 'thousands',
    },
    {
      label: "lines in the Hub's mirror",
      pattern: /\*\*([\d,]+) lines\*\* in the Hub/,
      expected: m.hub_mirror.lines === null ? null : thousands(m.hub_mirror.lines),
      precision: 'thousands',
    },
  ]
}

const NUMBER_WORDS = {
  fifteen: 15, sixteen: 16, seventeen: 17, eighteen: 18, nineteen: 19, twenty: 20,
  'twenty-one': 21, 'twenty-two': 22, 'twenty-three': 23, 'twenty-four': 24, 'twenty-five': 25,
}

function checkSpec(m) {
  // Collapsed to one line before matching. §73 is prose in a markdown file wrapped at a hundred
  // columns, so every one of these figures can have a line break anywhere inside it — and where a
  // paragraph wraps is not a fact about the system. A check that failed on a reflow would be a
  // check that taught people to stop reflowing.
  const text = readFileSync(SPEC, 'utf8').replace(/\s+/g, ' ')
  const failures = []
  for (const claim of claims(m)) {
    if (claim.expected === null) continue
    const match = claim.pattern.exec(text)
    if (!match) {
      failures.push(`${claim.label}: §73 no longer states this figure at all`)
      continue
    }
    const stated = claim.words
      ? NUMBER_WORDS[match[1].toLowerCase()]
      : Number(match[1].replace(/,/g, ''))
    if (stated === undefined) {
      failures.push(`${claim.label}: §73 says "${match[1]}", which this check cannot read as a number`)
      continue
    }
    if (stated !== claim.expected) {
      failures.push(
        `${claim.label}: §73 says ${match[1]}, measured ${claim.expected.toLocaleString('en-US')}`
        + (claim.precision === 'thousands' ? ' (rounded to the nearest thousand)' : ''))
    }
  }
  return failures
}

const measured = measure()

if (check) {
  const pinned = existsSync(PIN) ? readFileSync(PIN, 'utf8') : ''
  const fresh = `${JSON.stringify(measured, null, 2)}\n`
  const failures = []
  if (pinned !== fresh) {
    failures.push('reference/reports/scale.json is not what a run produces — '
      + 'run `node tools/scale-report.mjs` and read the diff')
  }
  failures.push(...checkSpec(measured))
  if (failures.length > 0) {
    console.error('§73 and the tree disagree:\n')
    for (const failure of failures) console.error(`  ${failure}`)
    console.error('\nThe table is the first thing a vendor reads. Correct the document, '
      + 'not this check.')
    process.exit(1)
  }
  console.log('§73 matches the tree.')
  for (const claim of claims(measured)) {
    if (claim.expected !== null) {
      console.log(`  ok  ${claim.label}: ${claim.expected.toLocaleString('en-US')}`)
    }
  }
  if (!hubMeasured) {
    console.log("  --  the Hub's mirror was not measured: no checkout beside this one")
  }
} else {
  writeFileSync(PIN, `${JSON.stringify(measured, null, 2)}\n`)
  console.log(`wrote ${PIN.slice(ROOT.length + 1)}`)
  console.log(`  rust      ${measured.rust.lines.toLocaleString('en-US')} lines, `
    + `${measured.rust.crates} crates, ${measured.rust.files} files`)
  console.log(`  reference ${measured.reference_typescript.lines.toLocaleString('en-US')} lines`)
  console.log(`  python    ${measured.editions.python_lines.toLocaleString('en-US')} lines`)
  console.log(`  drivers   ${measured.drivers.lines.toLocaleString('en-US')} lines`)
  console.log(`  corpus    ${measured.corpus.files} files, ${measured.corpus.cases} cases`)
  console.log(`  hub       ${measured.hub_mirror.lines?.toLocaleString('en-US') ?? 'unmeasured'}`
    + `${hubMeasured ? '' : ' (carried from the pin)'} lines`)
  console.log(`  editions  ${measured.editions.count}: ${measured.editions.ids.join(', ')}`)
}
