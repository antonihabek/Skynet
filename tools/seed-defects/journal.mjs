/**
 * A campaign's journal (DRY-COVERAGE-PLAN.md item 12.C.3).
 *
 * A campaign runs for hours and writes its report only when it ends, so a run the machine stopped — a power-off, a reboot, a
 * killed shell — left a log and nothing to resume from: Tello's, 251 seeds in, was lost that way on 2026-10-12 and had to be run
 * again from the first seed. With `--journal FILE` every conclusive verdict (killed, survived) is appended to FILE as it comes,
 * and a run begun with the same FILE takes those seeds from it instead of running them again.
 *
 * "The same" is checked, not assumed. The journal's first line says which driver, rung and seed set it was begun for, which of
 * the seeds were selected, the bytes of what the suite runs against (the driver's directory and what it links, but not prose or
 * caches: a README edited meanwhile changes nothing a seed can see), and the suite's own definition; its second says what the
 * unpatched suite came to, and the baseline runs again whatever the journal holds. Anything that differs refuses the resume and
 * says what, because a seed measured against other code is a seed measured against another driver. A verdict that is not a
 * conclusion (invalid, stale, flaky) is not kept, so that seed is run again. A campaign that ends renames its journal to
 * `FILE.done`, so a journal is only ever the record of a run that did not end, and a finished run is never "resumed" into a
 * report it did not measure.
 */
import { createHash } from 'node:crypto'
import { appendFileSync, existsSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const VERSION = 1

/** Not code and not data a suite reads: prose, caches, build output, the repository's own bookkeeping. */
const NOT_THE_SUITE = /(?:^|\/)(?:node_modules|target|\.target|\.git)(?:\/|$)|\.(?:pyc|md)$/

export class JournalRefusal extends Error {}

/** What a header key is called in the message that says it differs. */
const LABEL = {
  driver: 'driver',
  rung: 'rung',
  seeds: 'seed set',
  selection: 'selection of seeds',
  tree: 'suite code (the driver\'s directory and what it links)',
  suite: 'suite definition in suites.json',
}

function walk(root, path, out) {
  const stat = statSync(join(root, path), { throwIfNoEntry: false })
  if (!stat) return
  if (stat.isDirectory()) {
    for (const name of readdirSync(join(root, path)).sort()) walk(root, `${path}/${name}`, out)
  } else if (!NOT_THE_SUITE.test(path)) {
    out.push(path)
  }
}

/** A digest of the bytes of `paths` (files or directories, relative to `root`), leaving out prose and caches. */
export function treeDigest(root, paths) {
  const files = []
  for (const path of [...paths].sort()) walk(root, path, files)
  const hash = createHash('sha256')
  for (const file of files) {
    const bytes = readFileSync(join(root, file))
    hash.update(`${file}\0${bytes.length}\0`)
    hash.update(bytes)
  }
  return `${files.length}:${hash.digest('hex').slice(0, 16)}`
}

export function digestOfJson(value) {
  return createHash('sha256').update(JSON.stringify(value ?? null)).digest('hex').slice(0, 16)
}

/**
 * The rows of a journal. A power cut can leave half a line at the end, and that alone is forgiven (and cut off, so the next
 * row does not land on its tail); a line that is not JSON anywhere else means the file is not what this wrote.
 */
function readRows(path) {
  const text = readFileSync(path, 'utf8')
  const lines = text.split('\n')
  const rows = []
  let torn = false
  lines.forEach((line, index) => {
    if (line === '') return
    try {
      rows.push(JSON.parse(line))
    } catch {
      if (index !== lines.length - 1) throw new JournalRefusal(`${path} line ${index + 1} is not JSON, so this is not a journal this tool wrote (or it was edited); move it aside to start over.`)
      torn = true
    }
  })
  // A last row with no newline after it would have the next one written onto its end.
  if (torn || (text !== '' && !text.endsWith('\n'))) writeFileSync(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n')
  return rows
}

/**
 * Open the journal at `path` for a run described by `expected` ({driver, rung, seeds, selection, tree, suite}): a new file begins
 * one, an existing one must have been begun for exactly this run or it is refused.
 *
 * Returns `done` (seed id → what it came to, for the seeds that need not be run), `started` (when the journal was begun),
 * `append(id, result)`, `baselineIs(digest)` and `finish()`.
 */
export function openJournal(path, expected, now = () => new Date().toISOString()) {
  const done = new Map()
  let started
  let baseline
  if (existsSync(path)) {
    const [header, ...rest] = readRows(path)
    if (!header || header.journal !== VERSION) {
      throw new JournalRefusal(`${path} is not a campaign journal of this version (its first line is ${JSON.stringify(header ?? null).slice(0, 80)}); move it aside to start over.`)
    }
    const differs = Object.keys(expected).filter(key => header[key] !== expected[key])
    if (differs.length) {
      throw new JournalRefusal(`${path} was begun for another run than this one: its ${differs.map(key => `${LABEL[key] ?? key} is ${header[key]}, this run's is ${expected[key]}`).join('; ')}. A seed it holds was measured against code this run does not have, so none is taken from it. To begin this campaign over, delete it: rm ${path}`)
    }
    started = header.started
    for (const row of rest) {
      if (row.baseline !== undefined) baseline = row.baseline
      else if (row.id !== undefined) done.set(row.id, { verdict: row.verdict, detail: row.detail, seconds: row.seconds })
    }
  } else {
    started = now()
    writeFileSync(path, JSON.stringify({ journal: VERSION, ...expected, started }) + '\n')
  }
  return {
    done,
    started,
    /** Keep what a seed came to, if it came to a conclusion. */
    append(id, result) {
      if (result.verdict !== 'killed' && result.verdict !== 'survived') return
      appendFileSync(path, JSON.stringify({ id, verdict: result.verdict, detail: String(result.detail ?? '').slice(0, 400), seconds: Number((result.seconds ?? 0).toFixed(1)) }) + '\n')
    },
    /** What the unpatched suite came to must be what it came to when the journal's seeds were run. */
    baselineIs(digest) {
      if (baseline === undefined) {
        appendFileSync(path, JSON.stringify({ baseline: digest }) + '\n')
        baseline = digest
      } else if (baseline !== digest) {
        throw new JournalRefusal(`${path}: the unpatched suite's outcomes (${digest}) are not the ones the journal's seeds were measured against (${baseline}), so they would be compared with another baseline. To begin this campaign over, delete it: rm ${path}`)
      }
    },
    /** The run ended: the journal is its record, and cannot be resumed. */
    finish() {
      rmSync(`${path}.done`, { force: true })
      renameSync(path, `${path}.done`)
    },
  }
}
