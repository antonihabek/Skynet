#!/usr/bin/env node
/**
 * Which robots can install Skynet OS today, and how far each has been tested — generated, so it cannot
 * drift from the manifest it describes.
 *
 * # Why this exists
 *
 * The question an integrator asks first is "is there a driver for my robot, and how good is it?". The
 * answer was spread over `drivers/models.json` (a stage per driver, read by `skynet setup`),
 * `docs/status.md` (a paragraph per driver) and forty READMEs, and none of them said which *robots* a
 * driver covers. `drivers/robots.json` records that once, beside the manifest; this joins the two into
 * `docs/robot-support.md`'s tables.
 *
 * # What it refuses
 *
 * 1. A driver that commands a physical robot (a `prebuilt`, `sdk`, `cargo` or `python` model that is not
 *    `hardware: false`) with no record in `drivers/robots.json` saying which robots it drives, what kind
 *    of interface it speaks, and what its counterpart has not shown.
 * 2. A record for a model the manifest does not have; a body behind `ros2_control` with no human name;
 *    an interface that is not one of the three.
 * 3. A `docs/robot-support.md` whose generated blocks are not what this would write now.
 * 4. A name in that page's section 4 "Not driven" column with no row in section 5, where each claim that a
 *    robot cannot be driven is challenged.
 *
 *   node tools/robot-support.mjs           # rewrite the generated blocks
 *   node tools/robot-support.mjs --check   # fail if they are stale
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = (path) => readFileSync(join(ROOT, path), 'utf8').replace(/\r\n/g, '\n')
const DOC = 'docs/robot-support.md'
const STAGED_KINDS = ['prebuilt', 'sdk', 'cargo', 'python']
const INTERFACES = ['joint-level', 'command-level', 'hosted']

const manifest = JSON.parse(read('drivers/models.json')).models
const records = JSON.parse(read('drivers/robots.json')).drivers
const failures = []
const fail = (message) => failures.push(message)

const commands = (entry) => STAGED_KINDS.includes(entry.kind) && entry.hardware !== false

for (const [model, entry] of Object.entries(manifest)) {
  if (commands(entry) && !(model in records)) {
    fail(`drivers/robots.json has no record for \`${model}\`, which commands a physical robot: say which robots it drives, its interface, and what its counterpart has not shown.`)
  }
}
for (const [model, record] of Object.entries(records)) {
  const entry = manifest[model]
  if (!entry) {
    fail(`drivers/robots.json records \`${model}\`, which drivers/models.json does not have.`)
    continue
  }
  if (!INTERFACES.includes(record.interface)) fail(`${model}: interface ${JSON.stringify(record.interface)} is not one of ${INTERFACES.join(', ')}.`)
  if (!Array.isArray(record.unmeasured) || record.unmeasured.length === 0) fail(`${model}: says nothing about what is unmeasured, which is the half of the record that matters.`)
  if (!record.class) fail(`${model}: has no class.`)
  if (entry.bodies) {
    for (const body of Object.keys(entry.bodies)) {
      if (!record.bodies?.[body]) fail(`${model}/${body}: has no human name in drivers/robots.json.`)
    }
    for (const body of Object.keys(record.bodies ?? {})) {
      if (!(body in entry.bodies)) fail(`${model}/${body}: is named in drivers/robots.json and has no stage in drivers/models.json.`)
    }
  } else if (!Array.isArray(record.robots) || record.robots.length === 0) {
    fail(`${model}: names no robot.`)
  }
}

/** One row per robot: a body behind a hosting driver, or an entry of `robots`. */
function rows() {
  const out = []
  for (const [model, entry] of Object.entries(manifest)) {
    if (!commands(entry)) continue
    const record = records[model]
    if (!record) continue
    if (entry.bodies) {
      for (const [body, stageRecord] of Object.entries(entry.bodies)) {
        out.push({ robot: record.bodies?.[body] ?? body, model, interface: record.interface, stage: stageRecord.stage, rung: rungOf(stageRecord) })
      }
    } else {
      for (const robot of record.robots ?? []) {
        out.push({ robot, model, interface: record.interface, stage: entry.stage, rung: rungOf(entry) })
      }
    }
  }
  return out
}

function rungOf(record) {
  return record.stage === 'beta' ? record.substrate?.rung ?? '?' : record.stage === 'alpha' ? 4 : '-'
}

function summary(all) {
  const drivers = Object.entries(manifest).filter(([, entry]) => commands(entry))
  const count = { alpha: 0, beta: 0, stable: 0 }
  for (const row of all) count[row.stage] += 1
  const driverStages = { alpha: 0, beta: 0 }
  for (const [, entry] of drivers) {
    if (entry.bodies) for (const body of Object.values(entry.bodies)) driverStages[body.stage] += 1
    else driverStages[entry.stage] += 1
  }
  return (
    `**${all.length} robot rows across ${drivers.length} drivers that command a physical robot: ` +
    `${count.beta} beta, ${count.alpha} alpha, ${count.stable} stable.** ` +
    `(A row is one robot or one robot family a driver was built for or run against; a driver that hosts other vendors' plugins counts each body.)`
  )
}

function installable(all) {
  const lines = [
    '| Robot | Driver (`skynet setup --robot …`) | Interface | Stage | Rung |',
    '|---|---|---|---|---|',
  ]
  for (const row of all) {
    lines.push(`| ${row.robot} | \`${row.model}\` | ${row.interface} | **${row.stage}** | ${row.rung} |`)
  }
  return lines.join('\n')
}

function details() {
  const out = []
  for (const [model, entry] of Object.entries(manifest)) {
    if (!commands(entry)) continue
    const record = records[model]
    if (!record) continue
    out.push(`#### \`${model}\` — ${record.class}, ${record.interface}`)
    out.push('')
    if (entry.bodies) {
      out.push('Stage per body (each body runs a different vendor\'s plugin, so each has its own):')
      out.push('')
      for (const [body, stageRecord] of Object.entries(entry.bodies)) {
        out.push(`* **${record.bodies?.[body] ?? body}** — **${stageRecord.stage}**. ${stageRecord.tested_against}`)
      }
    } else {
      out.push(`**${entry.stage}.** ${entry.tested_against}`)
    }
    out.push('')
    out.push('Not shown by that counterpart:')
    out.push('')
    for (const item of record.unmeasured ?? []) out.push(`* ${item}`)
    out.push('')
  }
  return out.join('\n').trimEnd()
}

const all = rows()
const blocks = { summary: summary(all), installable: installable(all), details: details() }

const doc = read(DOC)

/**
 * Section 4 lists what is not driven, and section 5 is where each "cannot" is challenged. The two were
 * written apart once, and section 5 was promised and never written (`DRY-COVERAGE-PLAN.md` Phase 12), so
 * every name in section 4's "Not driven" column must appear in the first column of a section 5 row.
 */
function unansweredNames(text) {
  const four = text.indexOf('## 4. By class of robot')
  const five = text.indexOf('## 5. The claims that a driver cannot be written')
  const details = text.indexOf('<!-- generated:details -->')
  if (four < 0 || five < four || details < five) {
    return [`${DOC} needs "## 4. By class of robot", then "## 5. The claims that a driver cannot be written", then the generated details block, in that order.`]
  }
  const rowsOf = (part) =>
    part
      .split('\n')
      .filter((line) => line.startsWith('|') && !/^\|[-| ]+\|$/.test(line))
      .map((line) => line.slice(1, line.lastIndexOf('|')).split('|').map((cell) => cell.trim()))
  const header = rowsOf(text.slice(four, five)).find((row) => row[0] === 'Class')
  const column = header ? header.indexOf('Not driven') : -1
  if (column < 0) return [`${DOC} section 4 has no table with a "Not driven" column.`]
  const answered = rowsOf(text.slice(five, details))
    .map((row) => row[0].replace(/\*/g, '').toLowerCase())
    .join('\n')
  const missing = []
  for (const row of rowsOf(text.slice(four, five))) {
    if (row[0] === 'Class' || !row[column] || /^as above$/i.test(row[column])) continue
    let depth = 0
    let name = ''
    const names = []
    for (const ch of row[column]) {
      if (ch === '(') depth += 1
      if (ch === ')') depth -= 1
      if (ch === ',' && depth === 0) {
        names.push(name)
        name = ''
      } else {
        name += ch
      }
    }
    names.push(name)
    for (const raw of names) {
      const bare = raw.replace(/\s*\([^)]*\)/g, '').trim()
      if (bare && !answered.includes(bare.toLowerCase())) missing.push(bare)
    }
  }
  return missing.map(
    (name) =>
      `${DOC} section 4 lists \`${name}\` as not driven and section 5 has no row for it: add a row whose first cell names it, with a verdict and dated evidence, or the claim that it cannot be driven is unchallenged.`,
  )
}
for (const message of unansweredNames(doc)) fail(message)

let updated = doc
for (const [name, body] of Object.entries(blocks)) {
  const begin = `<!-- generated:${name} -->`
  const end = `<!-- /generated:${name} -->`
  const from = updated.indexOf(begin)
  const to = updated.indexOf(end)
  if (from < 0 || to < from) {
    fail(`${DOC} has no ${begin} … ${end} block.`)
    continue
  }
  updated = `${updated.slice(0, from + begin.length)}\n${body}\n${updated.slice(to)}`
}

if (failures.length > 0) {
  for (const failure of failures) console.error(`::error::${failure}`)
  console.error(`\n${failures.length} robot-support problem${failures.length === 1 ? '' : 's'}.`)
  process.exit(1)
}

if (process.argv.includes('--check')) {
  if (updated !== doc) {
    console.error(`::error::${DOC} is stale against drivers/models.json and drivers/robots.json. Run \`node tools/robot-support.mjs\` and commit the result.`)
    process.exit(1)
  }
  console.log(`${DOC} is current: ${all.length} robot rows.`)
} else {
  if (updated !== doc) writeFileSync(join(ROOT, DOC), updated)
  console.log(`${DOC} written: ${all.length} robot rows.`)
}
