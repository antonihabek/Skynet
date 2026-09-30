#!/usr/bin/env node
/**
 * The defect-class regression matrix (DRY-COVERAGE-PLAN.md item 12.C.2), generated from what the seeding campaign measured.
 *
 * # Why it is derived and not written
 *
 * The plan asked for "every historical finding class × every driver → has test / not applicable / missing". The obvious way to
 * fill that in is to look for a test that seems to cover the class, and a test that *seems* to cover a class is the thing this
 * whole phase distrusts: the xArm suite asserted that `close()` left the controller stopped, after a stop had already stopped
 * it. So a cell here is **has test** only if a defect of that class was seeded into the driver and the suite went red, and it
 * is **survives** when one was seeded and the suite stayed green. **Not applicable** is a sentence in `applicability.json`
 * saying why the class cannot happen to that driver, and **unseeded** is what is left: nobody has yet asked whether the suite
 * would notice, which is different from knowing it would not.
 *
 *   node tools/seed-defects/matrix.mjs           # write docs/dry-test-matrix.md
 *   node tools/seed-defects/matrix.mjs --check   # fail if it is stale, or the report it reads is not this seed set's
 *
 * What the matrix cannot say: it counts the seeds someone authored. A driver with every class green may still carry a defect
 * of a kind nobody thought of, and a suite's rung says how much the counterpart could show (F118), not that the counterpart
 * is right. It is a statement about the suite, and never about a robot.
 */
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { classes, digestOfSeeds, HERE, loadSeeds, readReport, REPO, suites } from './common.mjs'

const DOC = join(REPO, 'docs', 'dry-test-matrix.md')
const check = process.argv.includes('--check')
const failures = []
const fail = message => failures.push(message)

const all = loadSeeds()
const report = readReport()
const models = JSON.parse(readFileSync(join(REPO, 'drivers', 'models.json'), 'utf8')).models
const applicability = existsSync(join(HERE, 'applicability.json')) ? JSON.parse(readFileSync(join(HERE, 'applicability.json'), 'utf8')).drivers ?? {} : {}

const STAGED = ['prebuilt', 'sdk', 'cargo', 'python']
const hardware = Object.entries(models).filter(([, entry]) => STAGED.includes(entry.kind) && entry.hardware !== false).map(([name]) => name)
const seeded = Object.keys(all).sort()
const unseeded = hardware.filter(name => !all[name])

/** The strongest rung the report holds for a driver: rung 1 is the maker's own software, 4 a counterpart written here. */
function entries(driver) {
  return Object.entries(report.drivers?.[driver] ?? {}).map(([rung, entry]) => ({ rung: Number(rung), ...entry })).sort((a, b) => a.rung - b.rung)
}

// ── consistency: a matrix over a report that is not this seed set's says nothing ─────────────────
for (const driver of seeded) {
  const held = entries(driver)
  const rung4 = held.find(entry => entry.rung === 4)
  if (!rung4) { fail(`${driver} has ${all[driver].length} seeds and no rung 4 result in reference/reports/seed-defects.json. Run: node tools/seed-defects/seed-defects.mjs --driver ${driver} --report`); continue }
  const digest = digestOfSeeds({ [driver]: all[driver] })
  for (const entry of held) {
    if (entry.seedsDigest !== digest) fail(`${driver}: the rung ${entry.rung} result measured a different seed set (${entry.seedsDigest}) from today's (${digest}). Run it again with --report; a rate for seeds that no longer exist is not a rate.`)
    if (entry.mislabelledControls?.length) fail(`${driver} (rung ${entry.rung}): a seed marked to survive was killed (${entry.mislabelledControls.join(', ')}): it is not a control, or not the undetectable defect it says.`)
    if (entry.invalidSeeds?.length) fail(`${driver} (rung ${entry.rung}): seeds that do not compile measured nothing (${entry.invalidSeeds.join(', ')}). Fix or delete them.`)
    if (!entry.controls) fail(`${driver} (rung ${entry.rung}): no control seed took part, so its rate could not have fallen.`)
  }
}
for (const [driver, byClass] of Object.entries(applicability)) {
  if (!all[driver] && !unseeded.includes(driver)) fail(`applicability.json names ${driver}, which is not a hardware driver.`)
  for (const [name, reason] of Object.entries(byClass)) {
    // "*" is not a class: it is the one sentence for a driver that commands a robot and has no seeded suite at all.
    if (name !== '*' && !classes[name]) fail(`applicability.json: ${driver} names class "${name}", which classes.json does not define.`)
    if (typeof reason !== 'string' || reason.trim().length < 20) fail(`applicability.json: ${driver}/${name} says a class cannot happen without saying, in a sentence, why.`)
    if (all[driver]?.some(seed => seed.class === name)) fail(`applicability.json: ${driver}/${name} is "not applicable" and has a seed of that class. It is one or the other.`)
  }
}
for (const name of unseeded) {
  if (!suites[name] && !(applicability[name] && applicability[name]['*'])) {
    // A driver with no hermetic suite is said so once, in words, under the "*" key, rather than left out.
    fail(`${name} commands a robot and has neither seeds nor a stated reason for having none (applicability.json, "*").`)
  }
}

// ── the document ─────────────────────────────────────────────────────────────────────────────────
const CLASS_ORDER = Object.keys(classes).filter(name => name !== 'undetectable')
const percent = rate => rate === null || rate === undefined ? 'n/a' : `${(100 * rate).toFixed(0)}%`

function cell(driver, name) {
  const best = entries(driver)[0]
  if (!best) return '?'
  const counted = best.byClass?.[name]
  if (counted) return counted.killed === counted.seeds ? `**${counted.killed}/${counted.seeds}**` : `${counted.killed}/${counted.seeds}`
  if (applicability[driver]?.[name]) return '·'
  return '?'
}

const rows = seeded.map(driver => {
  const held = entries(driver)
  const best = held[0]
  const rung4 = held.find(entry => entry.rung === 4)
  const stage = models[driver]?.stage ?? '—'
  const measured = held.map(entry => `r${entry.rung} ${entry.killed}/${entry.seeds}`).join(', ')
  return { driver, stage, best, rung4, measured, held }
})

let out = `# Dry-test matrix — what each driver's suite notices

<!-- Generated by \`node tools/seed-defects/matrix.mjs\` from reference/reports/seed-defects.json, tools/seed-defects/ and
     tools/seed-defects/applicability.json. Do not edit: CI fails when this disagrees with them. -->

Item 12.C.2–3 of \`DRY-COVERAGE-PLAN.md\`. **This page measures the suites, not the robots.** For each driver, realistic defects — each
one of a kind that has really happened to a driver in this repository (\`tools/seed-defects/classes.json\`) — were put into a temporary
copy of it, one at a time, and the strongest suite that runs without hardware was run. A defect the suite goes red on is **noticed**; one
it does not is a **survivor**: a defect of that kind that would reach a robot. Every driver also carries a **control** — a defect no dry suite
can see — which must survive, so that a rate which cannot fall is not mistaken for a suite that cannot miss.

What the numbers are not: a probability that a driver works (no such number exists from here, and the plan refuses to print one); a
claim that no other defect is present (the seeds were authored, and the ones nobody thought of are not in the count); or a statement
about the counterpart the suite runs against, whose own correctness is what the release stage (\`alpha\`, \`beta\`) is about. **Rung** is
F118's: 1 the maker's controller software, 2 the maker's simulator, 3 an independent implementation, 4 a counterpart written in this
repository. A hermetic suite is rung 4 by construction, so a beta driver's hermetic rate understates it: the survivors are re-run at the
rung its stage rests on (\`--rung N --survivors-of 4\`) and the stronger result is the one in the class table.

Reproduce: \`node tools/seed-defects/seed-defects.mjs --driver <name> --report\` (WSL or Linux; \`SEED_PYTHON_REQUESTS\` and \`SEED_PYTHON_SPOT\` name
the interpreters that carry \`requests\` and \`bosdyn-client\`). \`--check\` says whether every seed still applies exactly once.

## Per driver

| Driver | Stage | Seeded | Noticed at rung 4 (hermetic) | Best rung measured | Noticed there | Controls that survived | Survivors there |
|---|---|---:|---:|---:|---:|---:|---:|
`
for (const { driver, stage, best, rung4 } of rows) {
  if (!best) continue
  out += `| \`${driver}\` | ${stage} | ${rung4.seeds} | ${rung4.killed} (${percent(rung4.rate)}) | ${best.rung} | ${best.killed} (${percent(best.rate)}) | ${best.controlsSurvived} of ${best.controls} | ${best.survived} |\n`
}
out += `\nMeasured, by rung: ${rows.map(row => `\`${row.driver}\` ${row.measured}`).join('; ')}.\n`

out += `
## By class of defect

Each cell is *noticed / seeded* for that class in that driver at the driver's strongest measured rung; **bold** is every one noticed. \`·\` is
not applicable (\`tools/seed-defects/applicability.json\` says why), and \`?\` is **unseeded**: nobody has yet asked whether the suite would
notice, which is not the same as knowing it would not. An unseeded cell is the to-do list.

| Class | Where it was found | ${seeded.map(driver => `\`${driver}\``).join(' | ')} |
|---|---|${seeded.map(() => '---:').join('|')}|
`
for (const name of CLASS_ORDER) {
  out += `| \`${name}\` | ${classes[name].finding} | ${seeded.map(driver => cell(driver, name)).join(' | ')} |\n`
}
const totals = seeded.map(driver => entries(driver)[0]).filter(Boolean)
const unseededCells = seeded.reduce((sum, driver) => sum + CLASS_ORDER.filter(name => cell(driver, name) === '?').length, 0)
const naCells = seeded.reduce((sum, driver) => sum + CLASS_ORDER.filter(name => cell(driver, name) === '·').length, 0)
out += `\n${totals.reduce((sum, entry) => sum + entry.seeds, 0)} seeded defects across ${seeded.length} drivers; ${unseededCells} class-by-driver cells unseeded and ${naCells} stated not applicable.\n`

out += `\n## Drivers that command a robot and have no hermetic suite here\n\n`
if (!unseeded.length) out += 'None.\n'
for (const name of unseeded) {
  out += `* \`${name}\` — ${applicability[name]?.['*'] ?? '(no reason recorded)'}\n`
}

out += `\n## Survivors\n\nEach row is a defect of a kind that has happened, put into the driver, that the driver's suite at its strongest measured rung did **not** notice. It is a\ntest to write, or — where the note says so — a defect that rung cannot observe and a higher one can. A survivor is a finding, not a score to defend.\n`
for (const { driver, best } of rows) {
  if (!best || !best.survivors.length) continue
  out += `\n### \`${driver}\` at rung ${best.rung}: ${best.survivors.length} survivor${best.survivors.length === 1 ? '' : 's'}\n\n| Seed | Class | The defect | Note |\n|---|---|---|---|\n`
  for (const survivor of best.survivors) out += `| \`${survivor.id}\` | \`${survivor.class}\` | ${survivor.why.replace(/\|/g, '\\|')} | ${(survivor.note ?? '').replace(/\|/g, '\\|')} |\n`
}
// A control is a defect a dry suite cannot see, and a survivor triaged as a real difference that no test at this rung can reach
// (a race with a window of microseconds, say) is one too: it is left out of the rate, and listed here so nobody reads the rate
// as covering it. One a suite then kills is reported as mislabelled, which is how a seam that makes it observable is noticed.
const unobservable = seeded.flatMap(driver => all[driver].filter(seed => seed.expect === 'survive' && !seed.equivalent).map(seed => ({ driver, ...seed })))
if (unobservable.length) {
  out += `
## Controls, and differences no test at the driver's rung can see (left out of every rate)

Each is a real difference that the suite is **expected** not to notice: a control is how a rate that cannot fall is caught (class \`undetectable\`), and any other row is a survivor triaged as unreachable at this rung, with the reason. They are listed so the rate is not read as covering them.

| Driver | Seed | Class | The difference | Why no test sees it |
|---|---|---|---|---|
`
  for (const seed of unobservable) out += `| \`${seed.driver}\` | \`${seed.id}\` | \`${seed.class}\` | ${seed.why.replace(/\|/g, '\|')} | ${(seed.note ?? (seed.class === 'undetectable' ? 'a control: only a real controller or a real clock could tell' : '')).replace(/\|/g, '\|')} |
`
}
const equivalents = seeded.flatMap(driver => (entries(driver)[0]?.equivalents ?? []).map(equivalent => ({ driver, ...equivalent })))
if (equivalents.length) {
  out += `\n## Equivalent mutants (left out of every rate)\n\nA change nothing observable differs by cannot be noticed by any test, so it is not a gap. Each carries its reason, and must still survive: one that a suite kills was not equivalent.\n\n| Driver | Seed | Class | Why nothing observable differs |\n|---|---|---|---|\n`
  for (const equivalent of equivalents) out += `| \`${equivalent.driver}\` | \`${equivalent.id}\` | \`${equivalent.class}\` | ${equivalent.reason.replace(/\|/g, '\\|')} |\n`
}

// ── write or check ───────────────────────────────────────────────────────────────────────────────
if (check) {
  const held = existsSync(DOC) ? readFileSync(DOC, 'utf8').replace(/\r\n/g, '\n') : ''
  if (held !== out) fail('docs/dry-test-matrix.md is not what tools/seed-defects/matrix.mjs writes now. Run: node tools/seed-defects/matrix.mjs')
  if (failures.length) {
    for (const message of failures) console.error(`  ${message}`)
    console.error(`\nmatrix: ${failures.length} problem(s).`)
    process.exit(1)
  }
  console.log(`docs/dry-test-matrix.md is current: ${totals.reduce((sum, entry) => sum + entry.seeds, 0)} seeded defects over ${seeded.length} drivers, each report measuring this seed set.`)
} else {
  if (failures.length) for (const message of failures) console.error(`  note: ${message}`)
  writeFileSync(DOC, out)
  console.log(`wrote docs/dry-test-matrix.md (${failures.length} problem(s) --check would refuse).`)
}
