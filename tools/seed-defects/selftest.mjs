#!/usr/bin/env node
/**
 * The gate is shown to fail before it is trusted to pass (CONTRIBUTING.md: a check comes with the case it exists to catch).
 *
 * `seed-defects.mjs --gate --baseline FILE` is what the nightly campaign ends with. This builds a committed-looking report from
 * the real one, then a series of "fresh" reports each made worse in one way the gate exists to catch, and asserts that the gate
 * refuses each for the reason it should, and accepts the unchanged report. No suite is run.
 *
 *   node tools/seed-defects/selftest.mjs
 */
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { HERE, REPORT } from './common.mjs'

const real = JSON.parse(readFileSync(REPORT, 'utf8'))
const dir = mkdtempSync(join(tmpdir(), 'seed-gate-'))
const failures = []

// A driver with a rung 4 run in which something was killed, so a survivor can be made from it.
const driver = Object.keys(real.drivers).find(name => real.drivers[name]['4']?.killed > 1 && real.drivers[name]['4'].verdicts)
if (!driver) {
  console.error('seed-defects selftest: the report has no driver with a rung 4 run to make worse; run a campaign with --report first.')
  process.exit(2)
}
const copy = () => JSON.parse(JSON.stringify(real))

function gate(name, fresh, expectRefusal) {
  const baseline = join(dir, 'baseline.json')
  const report = join(dir, `${name}.json`)
  writeFileSync(baseline, JSON.stringify(real))
  writeFileSync(report, JSON.stringify(fresh))
  const result = spawnSync('node', [join(HERE, 'seed-defects.mjs'), '--gate', '--baseline', baseline, '--driver', driver], {
    env: { ...process.env, SKYNET_SEED_REPORT: report },
    encoding: 'utf8',
  })
  const said = `${result.stdout}${result.stderr}`
  if (expectRefusal === null) {
    if (result.status !== 0) failures.push(`${name}: an unchanged report was refused:\n${said}`)
  } else if (result.status === 0) {
    failures.push(`${name}: the gate passed a report that ${expectRefusal.describe}.`)
  } else if (!expectRefusal.says.test(said)) {
    failures.push(`${name}: refused, but not for the reason it should (wanted ${expectRefusal.says}):\n${said}`)
  }
}

gate('unchanged', copy(), null)

{
  const fresh = copy()
  const run = fresh.drivers[driver]['4']
  const [id] = Object.entries(run.verdicts).find(([, verdict]) => verdict === 'killed')
  run.verdicts[id] = 'survived'
  run.killed -= 1
  run.survived += 1
  run.rate = Number((run.killed / run.seeds).toFixed(3))
  gate('lost-a-kill', fresh, { describe: `no longer notices ${id}`, says: /no longer notices|detection fell/ })
}

{
  const fresh = copy()
  const run = fresh.drivers[driver]['4']
  run.controlsSurvived = Math.max(0, run.controls - 1)
  gate('a-control-was-killed', fresh, { describe: 'lost a control', says: /controls survived/ })
}

{
  const fresh = copy()
  fresh.drivers[driver]['4'].seedsDigest = '0000000000000000'
  gate('other-seeds', fresh, { describe: 'measured another seed set', says: /different seed set/ })
}

{
  const fresh = copy()
  delete fresh.drivers[driver]['4']
  gate('driver-not-reached', fresh, { describe: 'did not run the driver at all', says: /did not reach it/ })
}

// The floor a driver has earned (`gated.json`) is read by `--check`, which is what the per-push job runs: a committed report that
// says less than a gated driver has already shown is refused without running any suite.
function floor(name, fresh, refusal) {
  const report = join(dir, `${name}.json`)
  writeFileSync(report, JSON.stringify(fresh))
  const result = spawnSync('node', [join(HERE, 'seed-defects.mjs'), '--check'], { env: { ...process.env, SKYNET_SEED_REPORT: report }, encoding: 'utf8' })
  const said = `${result.stdout}${result.stderr}`
  if (refusal === null) {
    if (result.status !== 0) failures.push(`${name}: a report at its floor was refused:
${said}`)
  } else if (result.status === 0) {
    failures.push(`${name}: --check passed a report that ${refusal.describe}.`)
  } else if (!refusal.says.test(said)) {
    failures.push(`${name}: refused, but not for the reason it should (wanted ${refusal.says}):
${said}`)
  }
}

const gated = JSON.parse(readFileSync(join(HERE, 'gated.json'), 'utf8'))
const held = gated.drivers.find(name => real.drivers[name]?.[String(gated.rung)]?.killed > 1)
if (!held) {
  failures.push('gated.json lists no driver with a rung run in the report, so the floor is held by nothing.')
} else {
  floor('floor-held', copy(), null)
  {
    const fresh = copy()
    const run = fresh.drivers[held][String(gated.rung)]
    run.killed -= 2
    run.survived += 2
    run.rate = Number((run.killed / run.seeds).toFixed(3))
    floor('below-the-floor', fresh, { describe: `says ${held} notices ${(100 * run.rate).toFixed(0)}%, under the floor it has met`, says: /fell below the detection floor/ })
  }
  {
    const fresh = copy()
    delete fresh.drivers[held][String(gated.rung)]
    floor('gated-and-unmeasured', fresh, { describe: `has no run of ${held}, which is gated`, says: /no run of it/ })
  }
}

// F223: `drivers/confidence.json` says when each driver was measured, and it reads that from the driver's own entry. Reading the report's
// file-wide `generated` instead made one driver's campaign move fourteen others' dates. So an entry with no date of its own is refused,
// and a different `generated` alone changes nothing.
function confidence(name, fresh, refusal) {
  const report = join(dir, `${name}.json`)
  writeFileSync(report, JSON.stringify(fresh))
  const result = spawnSync('node', [join(HERE, '..', 'check-confidence.mjs')], { env: { ...process.env, SKYNET_SEED_REPORT: report }, encoding: 'utf8' })
  const said = `${result.stdout}${result.stderr}`
  if (refusal === null) {
    if (result.status !== 0) failures.push(`${name}: check-confidence refused a report it should accept:\n${said}`)
  } else if (result.status === 0) {
    failures.push(`${name}: check-confidence passed a report that ${refusal.describe}.`)
  } else if (!refusal.says.test(said)) {
    failures.push(`${name}: refused, but not for the reason it should (wanted ${refusal.says}):\n${said}`)
  }
}

confidence('confidence-unchanged', copy(), null)
{
  const fresh = copy()
  fresh.generated = '1999-01-01'
  confidence('generated-alone-moves-nothing', fresh, null)
}
{
  const fresh = copy()
  delete fresh.drivers[driver]['4'].measured
  confidence('entry-without-a-date', fresh, { describe: `has no \`measured\` date on ${driver}`, says: /no `measured` date of its own/ })
}

rmSync(dir, { recursive: true, force: true })
if (failures.length) {
  for (const failure of failures) console.error(`::error::${failure}`)
  process.exit(1)
}
console.log(`seed-defects gate refuses 4 kinds of worse report on ${driver}, the floor check refuses 2 more on ${held}, and both accept the unchanged one; check-confidence refuses an entry with no date of its own and ignores the file's.`)
