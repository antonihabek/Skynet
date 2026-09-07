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

import { HUB_DIRECTORY_NAMES, findHub } from './hub-path.mjs'

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

/**
 * The Hub's mirror, measured, or `null` when there is no Hub checkout to measure.
 *
 * The candidate list is `hub-path.mjs`'s and not this file's, which is the fix for a defect this
 * function had: it looked for two spellings where the other two tools looked for seven, so on a
 * machine with the Hub at `../RobotOverflow` it reported the mirror unmeasured while
 * `check-mirrors.mjs` was comparing against that very tree. An unmeasured figure exits zero, so
 * the check was off and nothing said so.
 *
 * The mirror directory must exist, not merely the checkout: a Hub clone that has not yet been
 * synced is a tree this cannot measure, and measuring zero lines would be worse than saying so.
 */
function measureHubMirror() {
  const explicit = hubArg ? resolve(hubArg) : null
  const base = explicit !== null && existsSync(join(explicit, 'lib', 'skynet', 'os'))
    ? explicit
    : findHub(ROOT)
  if (base === null) return null
  const mirror = join(base, 'lib', 'skynet', 'os')
  if (!existsSync(mirror)) return null
  return { lines: lines(walk(mirror, ['.ts', '.tsx'])), measured: true, from: base }
}

/**
 * How many of §27's eight conditions this edition observes, read off the code that decides it.
 *
 * `skynetd_io::observed::Observed::of` builds one array of eight booleans and every runtime
 * sentence about the count is generated from it. Four *documents* also state the number, and
 * before this they stated it by hand: §73 in two rows, the README, and this crate's own header.
 * That number was wrong in the README for two rounds, and the build log recorded the same
 * correction three rounds running — which is the same signal that produced this whole file.
 *
 * Parsed rather than executed, because this is a Node script and the authority is Rust. What it
 * looks at is deliberately narrow: the literal `false` entries in that one array. An entry that is
 * `true` or a call — `kernel.observed()` — is a condition this daemon reads, and a condition that
 * gains a producer changes from `false` to a call in exactly that array. So the parse tracks the
 * decision rather than a comment near it, and it fails loudly if the array is not where it says.
 */
function measureConditions() {
  const source = readFileSync(
    join(ROOT, 'crates', 'skynetd-io', 'src', 'observed.rs'), 'utf8')
  // Anchored on the function, not on the field name: `struct Observed` declares `seen: [bool; 8]`
  // and an unanchored search finds that first, then reports one entry against eight conditions.
  const body = /pub fn of\([\s\S]*?seen:\s*\[([\s\S]*?)\],\s*\}/.exec(source)
  const array = body
  if (!array) {
    throw new Error(
      'tools/scale-report.mjs cannot find `Observed::of`\'s array in '
      + 'crates/skynetd-io/src/observed.rs. That array is the authority for how many of §27\'s '
      + 'eight conditions this edition observes; if it has moved, this parse has to move with it '
      + 'rather than be deleted.')
  }
  // One entry per line, each ending in a comma before its trailing comment.
  const entries = array[1]
    .split('\n')
    .map((line) => line.replace(/\/\/.*$/, '').trim())
    .filter((line) => line.length > 0)
    .map((line) => line.replace(/,$/, '').trim())
  const total = /CONDITIONS:\s*\[&str;\s*(\d+)\]/.exec(source)
  if (!total || entries.length !== Number(total[1])) {
    throw new Error(
      `observed.rs declares ${total ? total[1] : '?'} conditions and `
      + `Observed::of sets ${entries.length}. Those have to be the same number.`)
  }
  return {
    total: entries.length,
    observed: entries.filter((entry) => entry !== 'false').length,
    unobserved: entries.filter((entry) => entry === 'false').length,
  }
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
    conditions: measureConditions(),
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
  none: 0, one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, seven: 7, eight: 8,
  nine: 9, ten: 10,
  fifteen: 15, sixteen: 16, seventeen: 17, eighteen: 18, nineteen: 19, twenty: 20,
  'twenty-one': 21, 'twenty-two': 22, 'twenty-three': 23, 'twenty-four': 24, 'twenty-five': 25,
}

/**
 * The sentences that state how many of §27's conditions this edition does not observe.
 *
 * A separate pass from `claims`, and for two reasons rather than one.
 *
 * **Every occurrence has to match, not the first.** The figures in `claims` appear once each in one
 * paragraph. This one appears in §73's `§27` row, §73's `Audio` row, §81's closing paragraph and
 * the README — and the way it went wrong was that somebody corrected the row they were editing and
 * left the other three. A check that stopped at the first match would have passed on exactly that.
 *
 * **It spans two files.** The README is the first thing a vendor reads and it carried the figure
 * two rounds after it stopped being true.
 *
 * The count is the **unobserved** one because that is what all four sentences state. It is derived
 * from `Observed::of`, so a condition gaining a producer fails this check in four places at once
 * and the fix is to write the new number, not to remember that there was one.
 */
const CONDITION_SENTENCES = [
  {
    file: SPEC,
    label: '§73/§81',
    pattern: /([\w-]+) of (?:§27's eight|the eight) conditions are still (?:unobserved|the kernel's)/gi,
  },
  {
    file: SPEC,
    label: '§73',
    pattern: /([\w-]+) of the eight are still the kernel's/gi,
  },
  {
    file: join(ROOT, 'README.md'),
    label: 'README',
    pattern: /([\w-]+) of §27's eight conditions are unobserved/gi,
  },
]

function checkConditions(m) {
  const failures = []
  let found = 0
  for (const sentence of CONDITION_SENTENCES) {
    const text = readFileSync(sentence.file, 'utf8').replace(/\s+/g, ' ')
    for (const match of text.matchAll(sentence.pattern)) {
      found += 1
      const stated = NUMBER_WORDS[match[1].toLowerCase()]
      if (stated === undefined) {
        failures.push(
          `${sentence.label}: "${match[1]}" is not a number word this check can read`)
        continue
      }
      if (stated !== m.conditions.unobserved) {
        failures.push(
          `${sentence.label}: says ${match[1]} of §27's conditions are unobserved, `
          + `and Observed::of leaves ${m.conditions.unobserved}`)
      }
    }
  }
  // A figure nobody states any more is a figure this check has stopped guarding, and it would go
  // quiet rather than fail. The same argument `claims` makes when a pattern finds nothing.
  if (found === 0) {
    failures.push(
      "no document states how many of §27's conditions are unobserved any more, so this check "
      + 'is guarding nothing — restore the sentence or remove the check deliberately')
  }
  return failures
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
  failures.push(...checkConditions(measured))
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
    console.log(
      "  --  the Hub's mirror was not measured: no checkout beside this one"
      + ` (looked for ${HUB_DIRECTORY_NAMES.length} names, or pass the path)`)
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
