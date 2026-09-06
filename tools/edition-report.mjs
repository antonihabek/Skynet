#!/usr/bin/env node
/**
 * Measure every language edition against the corpus and write `reference/reports/editions.json`.
 *
 *   node tools/edition-report.mjs            # regenerate
 *   node tools/edition-report.mjs --check    # fail if the file on disk is not what a run produces
 *
 * # Why this file exists
 *
 * `/skynet/os/editions` is §32.4's second item, and it was deferred nine times with one reason:
 * *"needed the moment there is more than one"*, and there was one. There are two now, and the page
 * has a number to print per edition — which is exactly where a status page starts inventing things.
 *
 * §32.2's rule about the on-robot console is *"it must never show a value it did not receive"*, and
 * nothing about being the Hub makes that weaker. So the Hub does not carry a table of conformance
 * figures somebody typed; it carries this file, and this file is produced by running the editions.
 *
 * # What it deliberately does not contain
 *
 * **A timestamp.** A generated file that changes on every run is one no check can pin, and the
 * question "when was this measured" is answered by the commit that changed it. The file changes
 * when a number changes and at no other time, which is what makes `--check` meaningful in CI.
 *
 * # The matrix is read, never retyped
 *
 * §31.3's nine rows — ceiling, kernel, standing, and the reason for each — are the specification's,
 * and this file carries them only because it **parses them out of** `docs/skynet-os-architecture.md`
 * on every run. A second copy of a design table is how a Hub ends up describing an OS that does not
 * exist; a copy that is regenerated from the document, pinned, and checked in both repositories is
 * a view of it.
 *
 * The parse is strict and fails loudly. A table that has moved or changed shape stops this tool
 * rather than producing a page with six rows where the specification has nine.
 */

import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')
// Written under `reference/`, which is the directory whose whole purpose is "things the Hub keeps
// a copy of": the contracts, the wire fixtures, and now this. `check-contract-mirrors.mjs` carries
// it across and pins it, so the numbers the Hub renders are the numbers a run produced here.
const OUT = join(HERE, 'reference', 'reports', 'editions.json')
const check = process.argv.includes('--check')

/**
 * The editions that **exist**, with the command that measures each.
 *
 * An edition is on this list when there is code to run, not when there is a plan to write one.
 * §31.3's other six rows are in the specification and are absent here, which is the difference
 * between a matrix and an inventory.
 */
const EDITIONS = [
  {
    id: 'rust',
    language: 'Rust',
    standing: 'reference',
    path: 'crates/',
    runner: 'cargo run -q -p skynet-corpus --bin corpus-report',
    command: ['cargo', ['run', '-q', '-p', 'skynet-corpus', '--bin', 'corpus-report']],
  },
  {
    id: 'python',
    language: 'Python',
    standing: 'full',
    path: 'editions/python/',
    runner: 'python3 editions/python/run_corpus.py --json',
    command: ['python3', ['editions/python/run_corpus.py', '--json']],
  },
]

function measure(edition) {
  const [program, args] = edition.command
  let stdout
  try {
    stdout = execFileSync(program, args, { cwd: HERE, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  } catch (error) {
    // A runner that exits non-zero has not reproduced the corpus, and a report generated from it
    // would be a page reporting a passing edition that does not pass.
    console.error(`\n${edition.id}: \`${edition.runner}\` failed.`)
    console.error(error.stderr?.toString() ?? String(error))
    process.exit(1)
  }

  const result = JSON.parse(stdout)
  for (const field of ['passed', 'failed', 'unimplemented', 'total']) {
    if (typeof result[field] !== 'number') {
      console.error(`${edition.id}: the runner printed no \`${field}\``)
      process.exit(1)
    }
  }
  if (result.failed !== 0 || result.unimplemented !== 0) {
    console.error(
      `${edition.id}: ${result.failed} failed and ${result.unimplemented} unimplemented. ` +
        'An edition that has not reproduced the corpus is not an edition (§31.4), and this file ' +
        'is not the place to record that it half is.',
    )
    process.exit(1)
  }

  return {
    id: edition.id,
    language: edition.language,
    standing: edition.standing,
    path: edition.path,
    runner: edition.runner,
    cases_passed: result.passed,
    cases_total: result.total,
    // Only the Rust runner counts files; the Python one reports corpora. Both are printed where
    // they exist and neither is invented for the other.
    corpus_files: typeof result.files === 'number' ? result.files : null,
    corpora_complete: Array.isArray(result.complete) ? result.complete : null,
  }
}

/**
 * §31.3's table, parsed out of the specification.
 *
 * The document is mirrored byte-identically into the Hub and pinned, so it cannot change in one
 * repository alone — which makes it a source this can be read from rather than copied out of.
 */
function matrix() {
  const spec = readFileSync(join(HERE, 'docs', 'skynet-os-architecture.md'), 'utf8')
  const heading = spec.indexOf('### 31.3 The edition matrix')
  if (heading < 0) {
    console.error('§31.3 is not in docs/skynet-os-architecture.md under the heading this reads.')
    process.exit(1)
  }
  const header = spec.indexOf('| Edition | Ceiling | Kernel | Standing |', heading)
  if (header < 0) {
    console.error('§31.3 no longer opens with the five-column table this reads.')
    process.exit(1)
  }

  const rows = []
  const lines = spec.slice(header).split('\n')
  // Skip the header and the `|---|` rule, then take rows until the table ends.
  for (const line of lines.slice(2)) {
    if (!line.startsWith('|')) break
    const cells = line.split('|').slice(1, -1).map(cell => cell.trim())
    if (cells.length !== 5) {
      console.error(`§31.3 has a row with ${cells.length} cells: ${line}`)
      process.exit(1)
    }
    const [edition, ceiling, kernel, standing, why] = cells
    rows.push({
      // The specification bolds the edition and its standing; the emphasis is presentation and the
      // Hub does its own.
      edition: edition.replace(/\*\*/g, ''),
      ceiling: ceiling.replace(/\*\*/g, ''),
      kernel: kernel.replace(/\*\*/g, ''),
      standing: standing.replace(/\*\*/g, ''),
      why,
    })
  }

  if (rows.length < 5) {
    console.error(`§31.3's table parsed to ${rows.length} rows, which is not a matrix.`)
    process.exit(1)
  }
  return rows
}

/**
 * The TypeScript edition, which exists and **cannot be measured against the corpus**.
 *
 * `tools/corpus-gen/generate.ts` produces the corpus by *"running curated inputs through the
 * TypeScript oracle and freezing the canonical bytes of each"*. Measuring that oracle against its
 * own output would report 1,109 of 1,109 and mean nothing at all.
 *
 * It is here rather than left out because leaving it out is a lie of a different kind: the page
 * would say *no code written* about the edition this Hub is built in and the corpus is generated
 * from. Three states, then — measured, oracle, absent — and each is a different sentence.
 */
const ORACLE = {
  language: 'TypeScript',
  standing: 'oracle',
  path: 'reference/typescript/',
  why_unmeasured:
    'The corpus is generated from this edition, so measuring it against the corpus would be ' +
    'measuring it against its own output. What holds it honest is the other direction: every ' +
    'other edition reproducing what it produced, byte for byte.',
}

const editions = EDITIONS.map(measure)
const rows = matrix()

// Every edition that exists has to be a row of the matrix, or the page would show a measured
// edition the specification has never heard of. Matched on the language name, which is the column
// the specification keys on.
for (const edition of editions) {
  if (!rows.some(row => row.edition.toLowerCase() === edition.language.toLowerCase())) {
    console.error(
      `${edition.id}: §31.3 has no row for ${edition.language}. An edition the specification does ` +
        'not list is one the matrix has to gain before the Hub renders it.',
    )
    process.exit(1)
  }
}

// Every edition is measured against one corpus, and a report where two rows were measured against
// two versions of it would be a comparison of nothing. Asserted rather than assumed.
const totals = new Set(editions.map(edition => edition.cases_total))
if (totals.size > 1) {
  console.error(
    `the editions were measured against different corpora: ${[...totals].join(' vs ')} cases. ` +
      'Regenerate the corpus and run this again.',
  )
  process.exit(1)
}

// The oracle has to be a row of the matrix too, for the same reason a measured edition does.
if (!rows.some(row => row.edition.toLowerCase() === ORACLE.language.toLowerCase())) {
  console.error(`§31.3 has no row for ${ORACLE.language}, which is the edition the corpus comes from.`)
  process.exit(1)
}

const report = {
  cases: editions[0].cases_total,
  editions,
  oracle: ORACLE,
  matrix: rows,
  note:
    'Generated by tools/edition-report.mjs. Every number here is the output of running that ' +
    'edition against the corpus in this repository, and `matrix` is §31.3 parsed out of ' +
    'docs/skynet-os-architecture.md rather than retyped. The two halves answer different ' +
    'questions: the matrix is what the design says an edition of each language could be, and ' +
    '`editions` is what exists and has been run.',
}

const text = JSON.stringify(report, null, 2) + '\n'

if (check) {
  if (!existsSync(OUT)) {
    console.error(`${OUT} does not exist. Run this without --check.`)
    process.exit(1)
  }
  const held = readFileSync(OUT, 'utf8')
  if (held !== text) {
    console.error(
      '\nreference/reports/editions.json is not what running the editions produces.\n\n' +
        'Either an edition moved and the file was not regenerated, or the file was edited by ' +
        'hand. Run `node tools/edition-report.mjs` and commit the result.\n',
    )
    process.exit(1)
  }
  console.log(`reference/reports/editions.json matches: ${editions.length} editions, ${report.cases} cases each`)
} else {
  writeFileSync(OUT, text)
  console.log(`wrote reference/reports/editions.json — ${editions.length} editions, ${report.cases} cases each`)
  for (const edition of editions) {
    console.log(`  ${edition.id.padEnd(8)} ${edition.cases_passed} of ${edition.cases_total}`)
  }
}
