#!/usr/bin/env node
/**
 * The fleet report says what ran and no more — DRY-COVERAGE-PLAN.md Phase 10.
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

const STAGES = ['source', 'import', 'body', 'roles', 'conformance', 'loop', 'travel', 'hold', 'transport', 'skill']
const OUTCOMES = {
  source: ['pass', 'fail'],
  import: ['pass', 'refused', 'not_applicable', 'not_run'],
  body: ['pass', 'refused', 'fail', 'not_run'],
  roles: ['accepted', 'refused', 'not_applicable', 'not_run'],
  conformance: ['guest', 'none', 'fail', 'not_run'],
  loop: ['pass', 'fail', 'not_run'],
  travel: ['pass', 'fail', 'not_applicable', 'not_run'],
  hold: ['pass', 'fail', 'not_applicable', 'not_run'],
  transport: ['bound', 'refused', 'fail', 'not_applicable', 'not_run'],
  skill: ['bound', 'refused', 'fail', 'not_run'],
}
const RELEASE_WORDS = /\b(alpha|beta|stable)\b/

// The skills library the `skill` stage ran against (item 12.F.7), by name, as the report itself records it.
const library = new Map((report.skills ?? []).map((skill) => [skill.name, skill]))
const names = (text) => (typeof text === 'string' && text !== '' ? text.split(', ') : [])

/**
 * A `skill` row is lists of skill names, and every list is a claim the others must agree with.
 *
 * `binds`, `names` and `supplied` are the three questions the robot's own gate asks (`skynet skill check --body`); a skill *starts*
 * on a body only when all three hold, so `starts` is exactly their intersection and nothing else. A skill is *driven* only when its
 * action space binds, and a driven skill is in exactly one of `clean`, `clamped`, `host_refused` and `failed`. A body that reached
 * the loop was asked, so its stage is never `not_run` in a committed report (a runner without the Hub says so and the nightly
 * skips it, but a committed report is made by someone who has the library); and a body that did not reach the loop was not driven.
 */
function checkSkillStage(row, loop) {
  const skill = row.stages?.skill
  if (!skill) return
  if (loop?.outcome !== 'pass') {
    if (skill.outcome !== 'not_run') fail(`${row.id}: the skill stage ran on a body that did not reach the loop`)
    return
  }
  if (skill.outcome === 'not_run') {
    fail(`${row.id}: the skill stage did not run on a body that reached the loop (${skill.detail ?? 'no reason given'}): run fleet/sweep.py --stage skill with the Hub's skills`)
    return
  }
  if (library.size === 0) {
    fail('the report has skill rows and no `skills` block saying which library they ran against')
    return
  }
  const lists = {}
  for (const key of ['binds', 'names', 'supplied', 'starts', 'clean']) {
    lists[key] = new Set(names(skill[key]))
    for (const name of lists[key]) {
      if (!library.has(name)) fail(`${row.id}: skill.${key} names \`${name}\`, which is not in the library the report records`)
    }
  }
  for (const key of ['exceeds', 'clamped', 'host_refused', 'failed']) {
    lists[key] = new Set(Object.keys(skill[key] ?? {}))
    for (const name of lists[key]) {
      if (!library.has(name)) fail(`${row.id}: skill.${key} names \`${name}\`, which is not in the library the report records`)
    }
  }
  if (skill.skills !== library.size) fail(`${row.id}: the stage says ${skill.skills} skills and the library has ${library.size}`)
  const everyThree = [...lists.binds].filter((name) => lists.names.has(name) && lists.supplied.has(name)).sort()
  if (JSON.stringify([...lists.starts].sort()) !== JSON.stringify(everyThree)) {
    fail(`${row.id}: skill.starts is ${JSON.stringify([...lists.starts].sort())}; a skill starts only when it binds, names this body's joints and is supplied: ${JSON.stringify(everyThree)}`)
  }
  const driven = ['clean', 'clamped', 'host_refused', 'failed']
  const seen = new Map()
  for (const key of driven) {
    for (const name of lists[key]) {
      if (!lists.binds.has(name)) fail(`${row.id}: ${name} was driven (${key}) and its action space does not bind this body`)
      if (seen.has(name)) fail(`${row.id}: ${name} is in both ${seen.get(name)} and ${key}`)
      seen.set(name, key)
    }
  }
  // A skill that binds and has a taught table is driven; one that binds and commands nothing (a dwell) is not. The report's library says which.
  for (const name of lists.binds) {
    if (library.get(name)?.commands && !seen.has(name)) fail(`${row.id}: ${name} binds and commands, and the stage did not drive it`)
  }
  // `exceeds` is found from the plan, without the loop, so it is claimed of any skill that binds — driven or refused by the host.
  for (const name of lists.exceeds) {
    if (!lists.binds.has(name)) fail(`${row.id}: ${name} exceeds this body's travel and its action space does not bind it`)
  }
  if (skill.outcome === 'bound' && lists.binds.size === 0) fail(`${row.id}: the skill stage says bound and no skill's action space reaches this body`)
  if (skill.outcome === 'refused' && lists.binds.size > 0) fail(`${row.id}: the skill stage says refused and ${lists.binds.size} skill(s) bind`)
  if (skill.outcome === 'fail' && !(lists.failed.size > 0 && typeof skill.detail === 'string' && skill.detail.length > 0)) {
    fail(`${row.id}: the skill stage failed and does not say which skill or why`)
  }
  if (skill.outcome !== 'fail' && lists.failed.size > 0) fail(`${row.id}: ${lists.failed.size} skill(s) failed and the stage does not say fail`)
  for (const [name, why] of Object.entries(skill.host_refused ?? {})) {
    if (!['reorders joints', 'leaves a joint out'].includes(why)) fail(`${row.id}: ${name} is host_refused for \`${why}\`, which is not one of the host's two known limits`)
  }
}

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
  const { conformance, loop, travel, roles, hold } = row.stages
  if (roles?.outcome === 'accepted') {
    // An accepted table is the integrator's assertion checked against the body's own axes (item
    // 10.8): it must say which table, how many roles it gave and how many relations held, and the
    // roles must have reached the descriptor the driver served rather than stopped at this stage.
    if (!(roles.table && roles.assigned > 0 && roles.relations_held > 0)) {
      fail(`${row.id}: roles say accepted without a table, a role given and a relation held`)
    } else if (!(conformance?.roles >= roles.assigned)) {
      fail(`${row.id}: ${roles.assigned} roles were accepted and the served descriptor carries ${conformance?.roles}`)
    }
  }
  if (roles?.outcome === 'refused' && !(roles.table && roles.detail)) {
    fail(`${row.id}: roles say refused and do not say which table or why`)
  }
  if (conformance?.outcome === 'guest' && !(conformance.passed > 0)) {
    fail(`${row.id}: conformance says guest and counted no pass`)
  }
  if (loop?.outcome === 'pass' && !(loop.replayed > 0 && loop.complete === true)) {
    fail(`${row.id}: the loop passed without a complete, replayed window`)
  }
  if (travel?.outcome === 'pass' && !(travel.clamped >= travel.past_the_narrow_end && travel.past_the_narrow_end > 0)) {
    fail(`${row.id}: travel passed without clamping every request it counted`)
  }
  // The hold stage (item 12.C.7): a pass measured a body held for the whole run, and only a body that reached the loop was held.
  if (hold?.outcome === 'pass' && !(hold.joints > 0 && hold.seconds >= 2 && typeof hold.drift_rad === 'number' && typeof hold.speed_rad_s === 'number')) {
    fail(`${row.id}: the hold stage passed without a measurement of a held body (joints, seconds, drift, speed)`)
  }
  if (hold?.outcome === 'not_applicable') {
    // Never a way to hide a failure: a body is not applicable only because it has no floor to stand on, or because an independent hold of
    // the same law, written outside the driver, did not stand it either — and the row carries that hold's own measurement.
    const noFloor = /no geometry fixed to the world/.test(hold.detail ?? '')
    const refereed = /independent hold/.test(hold.detail ?? '') && typeof hold.reference?.drift_rad === 'number'
    if (!noFloor && !refereed) fail(`${row.id}: the hold stage is not_applicable and shows neither that the scene has no floor nor an independent hold that fell too`)
    if (loop?.outcome !== 'pass') fail(`${row.id}: the hold stage is not_applicable on a body that did not reach the loop`)
  }
  if (hold?.outcome === 'pass' && loop?.outcome !== 'pass') fail(`${row.id}: the hold stage passed on a body that did not reach the loop`)
  if (hold?.outcome === 'fail' && !(typeof hold.detail === 'string' && hold.detail.length > 0)) fail(`${row.id}: the hold stage failed and says nothing about why`)
  checkSkillStage(row, loop)
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

// The controls the hold bars are shown against: a body that lets go must fail them, or the bars cannot see what they exist to see.
const controls = report.controls?.hold
if (!Array.isArray(controls) || controls.length < 3) {
  fail('the report has no hold controls (at least three bodies that let go, run against the bars): run fleet/sweep.py')
} else {
  for (const control of controls) {
    if (control.outcome !== 'fail') {
      fail(`hold control ${control.id}: ${control.outcome}. A body that let go must fail the hold bars; one that does not says the bars cannot see a limp body`)
    }
  }
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
