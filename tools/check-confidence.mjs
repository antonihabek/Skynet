#!/usr/bin/env node
/**
 * The dry-confidence matrix (`drivers/confidence.json`, DRY-COVERAGE-PLAN.md item 12.C.1) says what evidence each
 * driver has at each of the ten layers `docs/robot-support.md` section 2 names, and this holds it to what can be checked
 * mechanically. It cannot check that a suite is *good*; it can check that the suite exists, is run where the row says,
 * agrees with the release stage, and does not claim a layer nothing dry can reach.
 *
 * # What it refuses
 *
 * 1. A driver that commands a physical robot with no row, and a row for one that does not.
 * 2. A suite whose command is not `make -C drivers/<dir> <target>` with that target in that Makefile — or whose workflow
 *    (`ci.yml` on every push, `substrates.yml` and `faults.yml` nightly) never runs it. A row that names a check nobody runs is a memory.
 *    A suite with no workflow says why in `manual`.
 * 3. A suite claiming layer 9 or 10, which are the hardware and the site and cannot be reached without a robot; or layer 7
 *    (timing at rate), which nothing here measures, unless the suite says `timing: true`; or layer 8 (version drift) from
 *    anything that is not run nightly.
 * 4. A layer that no suite reaches and no reason is given for — the omission that would read as coverage — and a layer that
 *    is both reached and excused.
 * 5. A stage that disagrees with the suites: `alpha` with a suite above rung 4, `beta` with no suite at rung 3 or better, or a
 *    beta whose `substrate` in `drivers/models.json` is not one of the suites at that rung.
 * 6. `detection` that is not what `reference/reports/seed-defects.json` says (the seeded-defect campaign's result for the
 *    driver's strongest measured rung), or present where there are no seeds. \`--sync\` writes it; the check compares.
 *
 *   node tools/check-confidence.mjs           # check
 *   node tools/check-confidence.mjs --sync    # write each driver's \`detection\` from the seed report
 *   node tools/check-confidence.mjs --verify [driver[/suite] ...]
 *                                             # run suites and stamp \`verified\` with today's date on each that exits 0
 *
 * The date beside a suite is the day its command last exited 0 *under this tool*, and nothing else: it is never typed. With no
 * names, \`--verify\` runs every suite a per-push workflow runs (\`ci: ci.yml\`) — those need no substrate — and says which ones
 * cannot run on this machine (a vendor SDK that is not installed is a red here, not a skipped green); name a substrate suite
 * and it runs that, which is docker and minutes. A suite that fails keeps the date it had, so the column says how stale it is.
 */
import { spawnSync } from 'node:child_process'
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = path => readFileSync(join(ROOT, path), 'utf8').replace(/\r\n/g, '\n')
// The environment override exists so a test can point the check at a deliberately broken copy.
const CONFIDENCE = process.env.SKYNET_CONFIDENCE ?? 'drivers/confidence.json'
const STAGED = ['prebuilt', 'sdk', 'cargo', 'python']
const WORKFLOWS = ['ci.yml', 'substrates.yml', 'faults.yml']
const sync = process.argv.includes('--sync')
const verifyAt = process.argv.indexOf('--verify')

const failures = []
const fail = message => failures.push(message)

const doc = JSON.parse(read(CONFIDENCE))
const models = JSON.parse(read('drivers/models.json')).models
const reportPath = 'reference/reports/seed-defects.json'
const report = existsSync(join(ROOT, reportPath)) ? JSON.parse(read(reportPath)) : { drivers: {} }
const seedsDir = join(ROOT, 'tools', 'seed-defects', 'seeds')
const seededDrivers = new Set(
  existsSync(seedsDir) ? readdirSync(seedsDir).filter(name => name.endsWith('.json')).map(name => JSON.parse(readFileSync(join(seedsDir, name), 'utf8')).driver) : [],
)

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

/** A workflow's steps: the text from one `- name:` line at step indentation to the next. */
function steps(file) {
  const path = join(ROOT, '.github', 'workflows', file)
  if (!existsSync(path)) return null
  const blocks = []
  let current = null
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    if (/^ {6}- (name|uses|run):/.test(line) || /^ {10}- (substrate|run):/.test(line)) {
      current = []
      blocks.push(current)
    }
    if (current) current.push(line)
  }
  return blocks.map(lines => lines.join('\n'))
}
const workflowSteps = Object.fromEntries(WORKFLOWS.map(file => [file, steps(file)]))
const workflowText = Object.fromEntries(WORKFLOWS.map(file => [file, existsSync(join(ROOT, '.github', 'workflows', file)) ? read(`.github/workflows/${file}`) : null]))

/** Whether some step of `file` runs `make -C drivers/<dir> <target>`, spelled in full or as `make <target>` in that directory. */
function runsIn(file, dir, target) {
  const text = workflowText[file]
  if (text === null) return false
  if (new RegExp(`make -C drivers/${dir}(?: [A-Za-z_]+=\\S+)* ${target}(?![\\w-])`).test(text)) return true
  return (workflowSteps[file] ?? []).some(step => step.includes(`working-directory: drivers/${dir}`) && new RegExp(`\\bmake (?:[A-Za-z_]+=\\S+ )*${target}(?![\\w-])`).test(step))
}

const today = new Date().toISOString().slice(0, 10)
const hardware = Object.entries(models).filter(([, entry]) => STAGED.includes(entry.kind) && entry.hardware !== false).map(([name]) => name)

for (const name of hardware) if (!doc.drivers[name]) fail(`${name} commands a physical robot and has no row in ${CONFIDENCE}: say what evidence it has at each of the ten layers, and what it has none for.`)
for (const name of Object.keys(doc.drivers)) if (!hardware.includes(name)) fail(`${CONFIDENCE} has a row for \`${name}\`, which drivers/models.json does not list as a driver that commands a robot.`)

// A link-chaos subject (`drivers/<dir>/chaos.py`, 12.C.4) is evidence, and evidence without a row is a number nobody is told about.
for (const dir of readdirSync(join(ROOT, 'drivers'))) {
  if (existsSync(join(ROOT, 'drivers', dir, 'chaos.py')) && !doc.drivers[dir]?.suites?.chaos) {
    fail(`drivers/${dir}/chaos.py is a link-chaos subject and ${CONFIDENCE} has no \`chaos\` suite for ${dir}: add one (run: make -C drivers/${dir} chaos, ci: faults.yml) so the nightly runs it and the matrix says so.`)
  }
}

/** A driver's strongest measured rung in the seed report, or null. */
function bestDetection(name) {
  const held = Object.entries(report.drivers?.[name] ?? {}).map(([rung, entry]) => ({ rung: Number(rung), entry })).sort((a, b) => a.rung - b.rung)[0]
  if (!held) return null
  return { rung: held.rung, noticed: held.entry.killed, seeded: held.entry.seeds, measured: report.generated }
}

for (const [name, row] of Object.entries(doc.drivers)) {
  const entry = models[name]
  if (!entry) continue
  const where = `${CONFIDENCE}: ${name}`
  const suites = Object.entries(row.suites ?? {})
  if (!suites.length) { fail(`${where} has no suites: a driver with no evidence at all has a stage the manifest should not give it.`); continue }
  const reached = new Map()

  for (const [id, suite] of suites) {
    const at = `${where}/${id}`
    if (![1, 2, 3, 4].includes(suite.rung)) fail(`${at}: rung ${JSON.stringify(suite.rung)} is not 1, 2, 3 or 4 (F118).`)
    if (typeof suite.what !== 'string' || suite.what.trim().length < 40) fail(`${at}: says nothing, in a sentence, of what it shows.`)
    const command = /^make -C drivers\/([A-Za-z0-9_-]+) ([A-Za-z0-9_-]+)$/.exec(suite.run ?? '')
    if (!command) { fail(`${at}: run is ${JSON.stringify(suite.run)}; it must be \`make -C drivers/<dir> <target>\`, the form the workflows run.`); continue }
    const [, dir, target] = command
    if (!makeTargets(dir).has(target)) fail(`${at}: \`${suite.run}\` names a target drivers/${dir}/Makefile does not define.`)
    if (!WORKFLOWS.includes(suite.ci) && suite.ci !== null) fail(`${at}: ci is ${JSON.stringify(suite.ci)}; it is ci.yml, substrates.yml, faults.yml, or null with a \`manual\` reason.`)
    if (suite.ci === null) {
      if (typeof suite.manual !== 'string' || suite.manual.trim().length < 20) fail(`${at}: is run by no workflow and does not say why (manual).`)
    } else if (!runsIn(suite.ci, dir, target)) {
      fail(`${at}: says ${suite.ci} runs \`${suite.run}\`, and no step of it does. A row that names a check nobody runs is a memory: add the step, or set ci to null and say why.`)
    }
    if (suite.verified !== null && (typeof suite.verified !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(suite.verified) || suite.verified > today)) fail(`${at}: verified is ${JSON.stringify(suite.verified)}; it is null, or the date of a run, and not later than today.`)
    if (!Array.isArray(suite.reaches) || !suite.reaches.length) { fail(`${at}: reaches no layer, so it is evidence of nothing.`); continue }
    for (const layer of suite.reaches) {
      if (!Number.isInteger(layer) || layer < 1 || layer > 10) { fail(`${at}: reaches layer ${JSON.stringify(layer)}, which is not one of the ten.`); continue }
      if (layer === 9 || layer === 10) fail(`${at}: claims layer ${layer} (${doc.layers[layer]}). No dry suite reaches it: that is what "dry" means, and a suite that says otherwise is the "looks like success" this project refuses.`)
      if (layer === 7 && !suite.timing) fail(`${at}: claims layer 7 (timing at rate) and is not marked \`timing: true\`. A simulator's clock is the host's, and nothing here measures a rate (12.C.5 measures the OS-side budget and says so).`)
      if (layer === 8 && suite.ci !== 'substrates.yml') fail(`${at}: claims layer 8 (version drift) and is not run nightly: version drift is found by re-running against what the maker ships, and only substrates.yml does.`)
      if (!reached.has(layer) || suite.rung < reached.get(layer)) reached.set(layer, suite.rung)
    }
  }

  // Every layer is reached, or excused in words; never neither, never both.
  const unreached = row.unreached ?? {}
  for (let layer = 1; layer <= 10; layer += 1) {
    const isReached = reached.has(layer)
    const excuse = unreached[layer] ?? (layer >= 6 ? doc.defaults?.[layer] : undefined)
    if (!isReached && !(typeof excuse === 'string' && excuse.trim().length >= 20)) fail(`${where}: layer ${layer} (${doc.layers[layer]}) is neither reached by a suite nor excused. Say which suite reaches it, or why none can.`)
    if (isReached && unreached[layer] !== undefined) fail(`${where}: layer ${layer} is reached by a suite and also listed as unreached.`)
  }
  for (const layer of Object.keys(unreached)) if (!doc.layers[layer]) fail(`${where}: unreached names layer ${layer}, which is not one of the ten.`)

  // Stage: what the manifest says a driver is, and what its suites are.
  const stageOf = record => record.stage
  if (entry.bodies) {
    for (const [body, record] of Object.entries(entry.bodies)) {
      if (stageOf(record) === 'beta') {
        const ran = suites.find(([, suite]) => suite.run === record.substrate?.run && suite.rung === record.substrate?.rung)
        if (!ran) fail(`${where}: body ${body} is beta on \`${record.substrate?.run}\` at rung ${record.substrate?.rung}, and no suite here is that.`)
      }
    }
  } else {
    const stage = stageOf(entry)
    const strongest = Math.min(...suites.map(([, suite]) => suite.rung))
    if (stage === 'alpha' && strongest < 4) fail(`${where}: is alpha and has a suite at rung ${strongest}. A driver re-measured against software this repository did not write is beta.`)
    if (stage === 'beta') {
      const ran = suites.find(([, suite]) => suite.run === entry.substrate?.run && suite.rung === entry.substrate?.rung)
      if (!ran) fail(`${where}: is beta on \`${entry.substrate?.run}\` at rung ${entry.substrate?.rung} (drivers/models.json), and no suite here is that.`)
    }
  }

  // Detection: what the seeded-defect campaign found, never typed by hand.
  const found = bestDetection(name)
  if (found && !seededDrivers.has(name)) fail(`${where}: the seed report has a result for ${name} and tools/seed-defects/seeds has no seeds for it.`)
  if (sync) {
    if (found) row.detection = found
    else delete row.detection
  } else if (found) {
    const held = row.detection
    if (!held || held.rung !== found.rung || held.noticed !== found.noticed || held.seeded !== found.seeded || held.measured !== found.measured) {
      fail(`${where}: detection ${JSON.stringify(held ?? null)} is not what the seed report says (${JSON.stringify(found)}). Run: node tools/check-confidence.mjs --sync`)
    }
  } else if (row.detection) {
    fail(`${where}: has a detection result and no seeded-defect run behind it.`)
  }
}

if (sync) {
  writeFileSync(join(ROOT, CONFIDENCE), `${JSON.stringify(doc, null, 1)}\n`)
  console.log(`${CONFIDENCE}: detection synced from ${reportPath}.`)
}
if (failures.length) {
  for (const message of failures) console.error(`::error::${message}`)
  console.error(`\ncheck-confidence: ${failures.length} problem${failures.length === 1 ? '' : 's'}.`)
  process.exit(1)
}
if (verifyAt >= 0) {
  const names = process.argv.slice(verifyAt + 1).filter(word => !word.startsWith('--'))
  const chosen = []
  for (const [name, row] of Object.entries(doc.drivers)) {
    for (const [id, suite] of Object.entries(row.suites)) {
      const named = names.some(word => word === name || word === `${name}/${id}`)
      if (names.length ? named : suite.ci === 'ci.yml') chosen.push({ name, id, suite })
    }
  }
  for (const word of names) if (!chosen.some(({ name, id }) => word === name || word === `${name}/${id}`)) { console.error(`::error::--verify ${word}: no such driver or suite in ${CONFIDENCE}.`); process.exit(1) }
  const red = []
  for (const { name, id, suite } of chosen) {
    const started = Date.now()
    const result = spawnSync('bash', ['-c', suite.run], { cwd: ROOT, encoding: 'utf8', maxBuffer: 1 << 28, timeout: 3600 * 1000 })
    const seconds = Math.round((Date.now() - started) / 1000)
    if (result.status === 0) {
      suite.verified = today
      console.log(`  green  ${name}/${id} (${seconds}s)  ${suite.run}`)
    } else {
      red.push(`${name}/${id}`)
      const tail = `${result.stdout ?? ''}${result.stderr ?? ''}`.trim().split('\n').slice(-6).join('\n      ')
      console.log(`  RED    ${name}/${id} (${seconds}s, exit ${result.status ?? result.signal})  ${suite.run}\n      ${tail}`)
    }
    // Written after each suite: a run of an hour that is interrupted keeps the greens it had.
    writeFileSync(join(ROOT, CONFIDENCE), `${JSON.stringify(doc, null, 1)}\n`)
  }
  console.log(`${chosen.length - red.length} of ${chosen.length} suites green and stamped ${today}${red.length ? `; red, date unchanged: ${red.join(', ')}` : ''}.`)
  if (red.length) process.exit(1)
}
const suiteCount = Object.values(doc.drivers).reduce((sum, row) => sum + Object.keys(row.suites).length, 0)
console.log(`${CONFIDENCE} is consistent: ${Object.keys(doc.drivers).length} drivers, ${suiteCount} suites, each run where it says, none claiming a layer nothing dry can reach.`)
