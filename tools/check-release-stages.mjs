#!/usr/bin/env node
/**
 * Every driver that commands a physical robot says how far it has been tested, and says it
 * truthfully: alpha, beta or stable, with what the stage rests on.
 *
 * # Why this exists
 *
 * DRY-COVERAGE-PLAN.md *Decisions* 19, the owner's instruction of 2026-09-25: no driver that has
 * not run on a real robot is described as working, and none is described as fake either. A driver
 * that was dry-tested as far as it can be is named by a release stage:
 *
 * | Stage | Means | Rests on |
 * |---|---|---|
 * | alpha | never run on a physical robot | counterparts written in this repository (F118's rung 4) |
 * | beta | never run on a physical robot | software this repository did not write that speaks the robot's own interface — the maker's controller software, the maker's simulator, or an independent implementation (F118's rungs 1–3) |
 * | stable | has run on a physical robot | a run nobody has made — *Decisions* 18 rules there will be no robot |
 *
 * The source of truth is `drivers/models.json`, because it is what `skynet setup --robot` reads and
 * prints at install time; `docs/status.md` restates it for readers. A stage written in two places
 * is two stages the first time one is edited, so this holds them to each other.
 *
 * # What it refuses
 *
 * 1. A model that commands a physical robot with no stage, or with a stage that is not one of the
 *    three, or with nothing saying what it was tested against. (`sim`, `host` and `library` models
 *    and `"hardware": false` bodies command nothing and must carry none.)
 * 2. `stable`, while this repository records no run on a physical robot. The day one exists, this
 *    check is where the record it needs gets defined — not a word someone typed.
 * 3. `beta` without a `substrate` — the command that re-runs it — or with one whose Makefile target
 *    does not exist, or that the nightly substrate workflow (`.github/workflows/substrates.yml`,
 *    Phase 8 item 8.11) does not run. A beta nobody re-measures is a beta that decays silently
 *    when the maker ships a release; this is what makes the label a measurement rather than a
 *    memory.
 * 4. A driver directory with no manifest entry, a manifest entry with no directory, and for a
 *    driver that hosts other vendors' code per body (`ros2_control`), a body with no description
 *    in the tree or a provisioned description with no body.
 * 5. A `docs/status.md` driver row that does not name its stage in bold, or names the other one.
 * 6. The word *fake* in the three places that describe drivers to the people who install them —
 *    `README.md`, `docs/status.md` and `drivers/models.json`. The test counterparts written here
 *    are described by what they are (a simulated controller written in this repository), and a
 *    driver is never called one.
 *
 *   node tools/check-release-stages.mjs
 */

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = (path) => readFileSync(join(ROOT, path), 'utf8')

const STAGED_KINDS = ['prebuilt', 'sdk', 'cargo', 'python']
const UNSTAGED_KINDS = ['sim', 'host', 'library', 'unmapped']
const STAGES = ['alpha', 'beta', 'stable']
const RECORD_FIELDS = ['stage', 'tested_against', 'substrate']
const FAKE = /\bfak(e|es|ed|ing)\b/i
const WORKFLOW = '.github/workflows/substrates.yml'

const failures = []
const fail = (message) => failures.push(message)

const manifest = JSON.parse(read('drivers/models.json'))
const models = manifest.models ?? {}
const workflow = existsSync(join(ROOT, WORKFLOW)) ? read(WORKFLOW) : null
if (workflow === null) {
  fail(`${WORKFLOW} is missing, so no beta driver is re-measured anywhere.`)
} else if (!/^\s*schedule:/m.test(workflow)) {
  fail(`${WORKFLOW} has no schedule, so "nightly" is a name and not a fact.`)
}

/** The targets a Makefile defines, by the plain `name:` lines make itself reads. */
function makeTargets(dir) {
  const path = join(ROOT, 'drivers', dir, 'Makefile')
  if (!existsSync(path)) return new Set()
  const targets = new Set()
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const match = /^([A-Za-z0-9_.-]+)\s*:(?!=)/.exec(line)
    if (match) targets.add(match[1])
  }
  return targets
}

/** One stage record — a model's, or one body's. */
function checkRecord(where, record) {
  const { stage, tested_against: testedAgainst, substrate } = record
  if (!STAGES.includes(stage)) {
    fail(`${where}: release stage is ${JSON.stringify(stage)}; it must be one of ${STAGES.join(', ')} (DRY-COVERAGE-PLAN.md Decisions 19).`)
    return
  }
  if (stage === 'stable') {
    fail(`${where}: "stable" means a run on a physical robot, and this repository records none (Decisions 18 and 19). Until a bench run is recorded and this check reads it, the most a driver can be is beta.`)
  }
  if (typeof testedAgainst !== 'string' || testedAgainst.trim() === '') {
    fail(`${where}: says it is ${stage} but not what it was tested against (tested_against).`)
  }
  if (stage === 'alpha' && substrate !== undefined) {
    fail(`${where}: is alpha and names a substrate. A substrate is software this repository did not write; a driver re-measured against one is beta. Drop the substrate or raise the stage.`)
  }
  if (stage !== 'beta') return
  if (substrate === undefined || typeof substrate !== 'object') {
    fail(`${where}: is beta with no substrate. Name the rung (1 the maker's controller software, 2 the maker's simulator, 3 an independent implementation) and the command that re-runs it.`)
    return
  }
  if (![1, 2, 3].includes(substrate.rung)) {
    fail(`${where}: substrate rung is ${JSON.stringify(substrate.rung)}; beta rests on rung 1, 2 or 3 (F118). Rung 4 — a counterpart written here — is alpha.`)
  }
  const run = /^make -C drivers\/([A-Za-z0-9_-]+) ([A-Za-z0-9_-]+)$/.exec(substrate.run ?? '')
  if (!run) {
    fail(`${where}: substrate.run is ${JSON.stringify(substrate.run)}; it must be \`make -C drivers/<dir> <target>\`, the form the nightly workflow runs.`)
    return
  }
  const [, dir, target] = run
  if (!makeTargets(dir).has(target)) {
    fail(`${where}: substrate.run names \`${target}\`, and drivers/${dir}/Makefile defines no such target.`)
  }
  if (workflow !== null && !workflow.includes(substrate.run)) {
    fail(`${where}: is beta on \`${substrate.run}\`, and ${WORKFLOW} never runs it. A beta nobody re-measures is not one — add it to the workflow's matrix.`)
  }
}

// ── 1–3: every model ────────────────────────────────────────────────────────────────────────────
const staged = new Map()
for (const [model, entry] of Object.entries(models)) {
  const { kind, dir } = entry
  if (![...STAGED_KINDS, ...UNSTAGED_KINDS].includes(kind)) {
    fail(`${model}: unknown kind ${JSON.stringify(kind)}.`)
    continue
  }
  if (entry.hardware !== undefined && !(entry.hardware === false && kind === 'prebuilt')) {
    fail(`${model}: "hardware" is only ever false, and only on a prebuilt body that drives nothing (the mock, the pendant).`)
  }
  const commandsHardware = STAGED_KINDS.includes(kind) && entry.hardware !== false
  const recordFields = [...RECORD_FIELDS, 'bodies'].filter((field) => field in entry)
  if (!commandsHardware) {
    if (recordFields.length > 0) {
      fail(`${model}: commands no physical robot (kind ${kind}${entry.hardware === false ? ', hardware false' : ''}) and carries ${recordFields.join(', ')}. A release stage is about a robot.`)
    }
    continue
  }
  if ('bodies' in entry) {
    const extra = RECORD_FIELDS.filter((field) => field in entry)
    if (extra.length > 0) {
      fail(`${model}: carries bodies and also ${extra.join(', ')} of its own. Per-body stages are the whole statement; a second one beside them is a second answer.`)
    }
    const bodies = Object.entries(entry.bodies ?? {})
    if (bodies.length === 0) fail(`${model}: bodies is empty.`)
    for (const [body, record] of bodies) checkRecord(`${model}/${body}`, record)
    staged.set(dir, [...new Set(bodies.map(([, record]) => record.stage))])
  } else if (!('stage' in entry)) {
    fail(`${model}: commands a physical robot and records no release stage. Every such driver is alpha, beta or stable (DRY-COVERAGE-PLAN.md Decisions 19).`)
  } else {
    checkRecord(model, entry)
    staged.set(dir, [entry.stage])
  }
}

// ── 4: the manifest and the tree agree ──────────────────────────────────────────────────────────
const driverDirs = readdirSync(join(ROOT, 'drivers')).filter((name) => {
  const path = join(ROOT, 'drivers', name)
  return statSync(path).isDirectory() && (existsSync(join(path, 'Makefile')) || existsSync(join(path, 'Cargo.toml')))
})
const mappedDirs = new Set(Object.values(models).map((entry) => entry.dir))
for (const dir of driverDirs) {
  if (!mappedDirs.has(dir)) fail(`drivers/${dir} has no entry in drivers/models.json, so \`skynet setup --robot\` cannot name it and nothing says how far it was tested.`)
}
for (const [model, entry] of Object.entries(models)) {
  if (!existsSync(join(ROOT, 'drivers', entry.dir ?? ''))) fail(`${model}: drivers/${entry.dir} does not exist.`)
}
for (const [model, entry] of Object.entries(models)) {
  if (!entry.bodies) continue
  const urdfDir = join(ROOT, 'drivers', entry.dir, 'urdf')
  const files = existsSync(urdfDir) ? readdirSync(urdfDir) : []
  for (const body of Object.keys(entry.bodies)) {
    const described = files.some((file) => file === `${body}.provisioned.json` || (file.startsWith(`${body}_`) && file.endsWith('.urdf')))
    if (!described) fail(`${model}/${body}: nothing in drivers/${entry.dir}/urdf/ describes this body (no ${body}.provisioned.json and no ${body}_*.urdf).`)
  }
  for (const file of files.filter((name) => name.endsWith('.provisioned.json'))) {
    const body = file.slice(0, -'.provisioned.json'.length)
    if (!(body in entry.bodies)) fail(`drivers/${entry.dir}/urdf/${file} is a provisioned body with no stage in drivers/models.json (${model}.bodies.${body}).`)
  }
}

// ── 5: docs/status.md says the same stage ───────────────────────────────────────────────────────
const status = read('docs/status.md')
const rows = status.split('\n').filter((line) => line.startsWith('| `drivers/'))
for (const [dir, stages] of staged) {
  const row = rows.find((line) => line.split('|')[1].includes(`\`drivers/${dir}\``))
  if (!row) {
    fail(`docs/status.md has no row naming \`drivers/${dir}\`.`)
    continue
  }
  for (const stage of stages) {
    if (!row.includes(`**${stage}**`)) fail(`docs/status.md: the row for drivers/${dir} does not say **${stage}**, which drivers/models.json records.`)
  }
  for (const other of STAGES.filter((stage) => !stages.includes(stage))) {
    if (row.includes(`**${other}**`)) fail(`docs/status.md: the row for drivers/${dir} says **${other}**, and drivers/models.json records ${stages.join(' and ')}.`)
  }
}

// ── 6: nothing that describes a driver to its user calls anything fake ──────────────────────────
for (const path of ['README.md', 'docs/status.md', 'drivers/models.json']) {
  read(path).split('\n').forEach((line, index) => {
    if (FAKE.test(line)) {
      fail(`${path}:${index + 1} says "${FAKE.exec(line)[0]}". Name the counterpart for what it is — a simulated controller written in this repository — and the driver by its release stage (Decisions 19).`)
    }
  })
}

if (failures.length > 0) {
  for (const failure of failures) console.error(`::error::${failure}`)
  console.error(`\n${failures.length} release-stage problem${failures.length === 1 ? '' : 's'}.`)
  process.exit(1)
}
const drivers = { alpha: 0, beta: 0 }
const bodies = { alpha: 0, beta: 0 }
for (const entry of Object.values(models)) {
  if (entry.bodies) for (const record of Object.values(entry.bodies)) bodies[record.stage] += 1
  else if (entry.stage) drivers[entry.stage] += 1
}
console.log(`Release stages hold: ${drivers.beta} drivers beta and ${drivers.alpha} alpha, ${bodies.beta} bodies beta and ${bodies.alpha} alpha behind drivers that stage per body, none stable; every beta re-run by ${WORKFLOW}.`)
