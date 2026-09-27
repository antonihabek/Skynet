#!/usr/bin/env node
/**
 * The fleet report says what ran and no more — DRY-COVERAGE-PLAN.md Phase 9.
 *
 * `fleet/sweep.py` runs every public robot description through the OS and writes
 * `reference/reports/fleet.json`. Three documents then make claims from it — the catalogue that pins
 * what was run, `docs/status.md` and the README — and a claim about a robot that nobody can check is
 * the thing this repository spends its CI on. So this holds them to each other, mechanically:
 *
 * 1. **The catalogue and the report agree.** Every catalogue entry has a row and every row an
 *    entry, with the same maker, model, class and route. A robot added to one and not run is caught
 *    rather than silently absent from a figure.
 * 2. **A route is a driver this tree has.** `drivers/<dir>`, or `ros2_control:<body>` naming a body
 *    in `drivers/models.json`. The report never states a release stage itself — a row carrying one
 *    is refused — so the fleet can never promote a driver (*Decisions* 19).
 * 3. **An outcome is one of the closed set, and a pass executed.** A `guest` conformance row counted
 *    passes; a `pass` loop replayed its window and said the window was complete; a `pass` travel row
 *    clamped every request it counted. §15's standing rule: a dry run may report `pass` only for a
 *    check that executed.
 * 4. **The summary is the rows.** Every count in `summary` is recomputed from the rows and compared.
 * 5. **The prose says the report's numbers.** `docs/status.md` and the README state the fleet's
 *    headline figures in a sentence each; the figures in those sentences are read and compared.
 *
 *   node tools/check-fleet.mjs
 */

import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = (path) => readFileSync(join(ROOT, path), 'utf8')
const failures = []
const fail = (message) => failures.push(message)

const catalogue = JSON.parse(read('fleet/catalogue.json')).entries
const report = JSON.parse(read('reference/reports/fleet.json'))
const models = JSON.parse(read('drivers/models.json')).models

const STAGES = ['source', 'import', 'body', 'conformance', 'loop', 'travel', 'transport']
const OUTCOMES = {
  source: ['pass', 'fail'],
  import: ['pass', 'refused', 'not_applicable', 'not_run'],
  body: ['pass', 'refused', 'fail', 'not_run'],
  conformance: ['guest', 'none', 'fail', 'not_run'],
  loop: ['pass', 'fail', 'not_run'],
  travel: ['pass', 'fail', 'not_applicable', 'not_run'],
  transport: ['bound', 'refused', 'fail', 'not_applicable', 'not_run'],
}
const RELEASE_WORDS = /\b(alpha|beta|stable)\b/

// 1 — the catalogue and the report.
const rows = new Map(report.robots.map((row) => [row.id, row]))
const entries = new Map(catalogue.map((entry) => [entry.id, entry]))
for (const entry of catalogue) {
  const row = rows.get(entry.id)
  if (!row) {
    fail(`${entry.id} is in fleet/catalogue.json and has no row in the report — run fleet/sweep.py`)
    continue
  }
  for (const key of ['maker', 'model', 'class', 'route']) {
    if (row[key] !== entry[key]) {
      fail(`${entry.id}: the report says ${key} ${JSON.stringify(row[key])}, the catalogue ${JSON.stringify(entry[key])}`)
    }
  }
}
for (const id of rows.keys()) {
  if (!entries.has(id)) fail(`${id} has a report row and no catalogue entry`)
}

// 2 — routes, and no release stage in the report.
for (const entry of catalogue) {
  if (entry.route === null) continue
  const [dir, body] = entry.route.split(':')
  if (!existsSync(join(ROOT, 'drivers', dir))) {
    fail(`${entry.id} routes to drivers/${dir}, which does not exist`)
  } else if (!Object.values(models).some((model) => model.dir === dir)) {
    fail(`${entry.id} routes to drivers/${dir}, which drivers/models.json does not map`)
  }
  if (body !== undefined && !(models[dir]?.bodies && body in models[dir].bodies)) {
    fail(`${entry.id} routes to ${dir} body ${body}, which drivers/models.json does not list`)
  }
}
for (const row of report.robots) {
  if ('stage' in row) fail(`${row.id} carries a release stage; that lives in drivers/models.json only`)
}

// 3 — outcomes, and passes that executed.
for (const row of report.robots) {
  for (const stage of STAGES) {
    const result = row.stages?.[stage]
    if (!result) {
      fail(`${row.id} has no ${stage} stage`)
      continue
    }
    if (!OUTCOMES[stage].includes(result.outcome)) {
      fail(`${row.id}: ${stage} is ${JSON.stringify(result.outcome)}, not one of ${OUTCOMES[stage].join(', ')}`)
    }
  }
  const { conformance, loop, travel } = row.stages
  if (conformance?.outcome === 'guest' && !(conformance.passed > 0)) {
    fail(`${row.id}: conformance says guest and counted no pass`)
  }
  if (loop?.outcome === 'pass' && !(loop.replayed > 0 && loop.complete === true)) {
    fail(`${row.id}: the loop passed without a complete, replayed window`)
  }
  if (travel?.outcome === 'pass' && !(travel.clamped >= travel.past_the_narrow_end && travel.past_the_narrow_end > 0)) {
    fail(`${row.id}: travel passed without clamping every request it counted`)
  }
  if (RELEASE_WORDS.test(JSON.stringify(row.stages))) {
    fail(`${row.id}: a stage's text uses a release-stage word; the fleet does not stage drivers`)
  }
}

// 4 — the summary is the rows.
const summary = report.summary
const recount = Object.fromEntries(STAGES.map((stage) => [stage, {}]))
for (const row of report.robots) {
  for (const stage of STAGES) {
    const outcome = row.stages?.[stage]?.outcome ?? 'not_run'
    recount[stage][outcome] = (recount[stage][outcome] ?? 0) + 1
  }
}
for (const stage of STAGES) {
  const said = JSON.stringify(Object.entries(summary.stages?.[stage] ?? {}).sort())
  const found = JSON.stringify(Object.entries(recount[stage]).sort())
  if (said !== found) fail(`summary.stages.${stage} says ${said}; the rows say ${found}`)
}
if (summary.descriptions !== report.robots.length) {
  fail(`summary.descriptions is ${summary.descriptions}; there are ${report.robots.length} rows`)
}
const robots = new Set(report.robots.map((row) => `${row.maker}\u0000${row.model}`))
if (summary.robots !== robots.size) fail(`summary.robots is ${summary.robots}; the rows name ${robots.size}`)
const through = new Set(
  report.robots
    .filter((row) => row.stages.conformance?.outcome === 'guest' && row.stages.loop?.outcome === 'pass')
    .map((row) => `${row.maker}\u0000${row.model}`),
)
if (summary.robots_through_every_stage !== through.size) {
  fail(`summary.robots_through_every_stage is ${summary.robots_through_every_stage}; the rows give ${through.size}`)
}

// 5 — the prose.
const SENTENCE = /(\d+) public robot descriptions? of (\d+) robots?[^.]*?(\d+) (?:robots? )?through (?:the )?conformance and the kernel loop/
for (const path of ['docs/status.md', 'README.md']) {
  const found = read(path).replace(/\s+/g, ' ').match(SENTENCE)
  if (!found) {
    fail(`${path} does not state the fleet's figures in the sentence this check reads (${SENTENCE})`)
    continue
  }
  const [descriptions, count, passed] = found.slice(1, 4).map(Number)
  if (descriptions !== summary.descriptions || count !== summary.robots || passed !== summary.robots_through_every_stage) {
    fail(
      `${path} says ${descriptions} descriptions of ${count} robots, ${passed} through; the report says ` +
        `${summary.descriptions}, ${summary.robots}, ${summary.robots_through_every_stage}`,
    )
  }
}

if (failures.length) {
  for (const message of failures) console.error(`check-fleet: ${message}`)
  process.exit(1)
}
console.log(
  `fleet: ${summary.descriptions} descriptions of ${summary.robots} robots; ${summary.robots_through_every_stage} ` +
    `through conformance and the kernel loop; report, catalogue, manifest and prose agree`,
)
