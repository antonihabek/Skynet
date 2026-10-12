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
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { HERE, REPORT } from './common.mjs'
import { JournalRefusal, openJournal, treeDigest } from './journal.mjs'

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

// The journal (journal.mjs): a campaign the machine stopped is resumed, and only when it is the same campaign. Tello's, 251 seeds in, was
// lost when the machine was powered off (2026-10-12), because the report is written when a run ends; what each seed came to is now kept as
// it comes. A journal that resumed a run it was not begun for would put seeds measured against other code into a report, so every way the
// run can differ is refused and named, and the file is repaired where a power cut can have damaged it and nowhere else.
const expected = { driver: 'demo', rung: 4, seeds: 'aaaa', selection: 'bbbb', tree: '3:cccc', suite: 'dddd' }
const journalFile = name => join(dir, `${name}.journal`)
const ids = journal => [...journal.done.keys()].sort().join(',')

/** A test that throws is a failure that says so, not a crash that hides the ones after it. */
function section(name, body) {
  try {
    body()
  } catch (error) {
    failures.push(`${name}: threw ${error?.stack ?? error}`)
  }
}

function refused(name, attempt, says) {
  try {
    attempt()
  } catch (error) {
    if (!(error instanceof JournalRefusal)) failures.push(`${name}: refused with something that is not a refusal: ${error?.stack ?? error}`)
    else if (!says.test(error.message)) failures.push(`${name}: refused, but not for the reason it should (wanted ${says}): ${error.message}`)
    return
  }
  failures.push(`${name}: the journal accepted what it should have refused (wanted ${says}).`)
}

section('resume', () => {
  const path = journalFile('resume')
  const first = openJournal(path, expected)
  if (first.done.size !== 0) failures.push('resume: a journal just begun says it holds seeds.')
  first.baselineIs('base-1')
  first.append('s1', { verdict: 'killed', detail: 'FAIL a thing', seconds: 1.234 })
  first.append('s2', { verdict: 'survived', detail: '', seconds: 2 })
  first.append('s3', { verdict: 'invalid', detail: 'does not compile', seconds: 0.1 })
  first.append('s4', { verdict: 'stale', detail: '', seconds: 0 })
  const second = openJournal(path, expected)
  if (ids(second) !== 's1,s2') failures.push(`resume: a journal begun for this run held ${ids(second) || 'nothing'}, wanted s1,s2 (a seed that came to no conclusion is run again).`)
  if (second.done.get('s1')?.verdict !== 'killed' || second.done.get('s1')?.detail !== 'FAIL a thing') failures.push(`resume: s1 came back as ${JSON.stringify(second.done.get('s1'))}.`)
  second.baselineIs('base-1')
  refused('other-baseline', () => second.baselineIs('base-2'), /not the ones the journal's seeds were measured against/)
  for (const [key, says] of [['driver', /driver is demo/], ['rung', /rung is 4/], ['seeds', /seed set is aaaa/], ['selection', /selection of seeds is bbbb/], ['tree', /suite code .* is 3:cccc/], ['suite', /suite definition in suites.json is dddd/]]) {
    refused(`other-${key}`, () => openJournal(path, { ...expected, [key]: key === 'rung' ? 1 : 'zzzz' }), says)
  }
  refused('other-run-names-the-way-out', () => openJournal(path, { ...expected, seeds: 'zzzz' }), /rm .*resume\.journal/)
})

section('torn', () => {
  // A power cut leaves half a row, or a whole row without its newline: either is repaired, and the row written next is not lost on its tail.
  const path = journalFile('torn')
  const first = openJournal(path, expected)
  first.append('t1', { verdict: 'killed', detail: 'x', seconds: 1 })
  appendFileSync(path, '{"id":"t2","verdict":"kil')
  const second = openJournal(path, expected)
  if (ids(second) !== 't1') failures.push(`torn: a half row was kept or the row before it lost: ${ids(second) || 'nothing'}, wanted t1.`)
  second.append('t3', { verdict: 'survived', detail: '', seconds: 1 })
  if (ids(openJournal(path, expected)) !== 't1,t3') failures.push('torn: the row written after a half row was lost (it landed on the half row\'s tail).')
  appendFileSync(path, '{"id":"t4","verdict":"killed","detail":"y","seconds":1}')
  const third = openJournal(path, expected)
  if (ids(third) !== 't1,t3,t4') failures.push(`torn: a whole row without its newline was lost: ${ids(third)}, wanted t1,t3,t4.`)
  third.append('t5', { verdict: 'killed', detail: 'z', seconds: 1 })
  if (ids(openJournal(path, expected)) !== 't1,t3,t4,t5') failures.push('torn: the row written after a row with no newline was lost.')
})

section('corrupt', () => {
  const path = journalFile('corrupt')
  openJournal(path, expected).append('c1', { verdict: 'killed', detail: '', seconds: 1 })
  appendFileSync(path, 'this is not json\n{"id":"c2","verdict":"killed","detail":"","seconds":1}\n')
  refused('corrupt-row', () => openJournal(path, expected), /line 3 is not JSON/)
  writeFileSync(path, '{"drivers":{}}\n')
  refused('not-a-journal', () => openJournal(path, expected), /not a campaign journal/)
  writeFileSync(path, '')
  refused('empty-file', () => openJournal(path, expected), /not a campaign journal/)
})

section('finish', () => {
  const path = journalFile('finish')
  const run = openJournal(path, expected)
  run.append('f1', { verdict: 'killed', detail: '', seconds: 1 })
  run.finish()
  if (existsSync(path) || !existsSync(`${path}.done`)) failures.push('finish: a campaign that ended did not leave its journal as `.done` and nothing else.')
  const again = openJournal(path, expected)
  if (again.done.size !== 0) failures.push('finish: a finished campaign\'s journal was resumed into the next run.')
  again.append('f2', { verdict: 'killed', detail: '', seconds: 1 })
  again.finish()
  if (!/"id":"f2"/.test(readFileSync(`${path}.done`, 'utf8')) || /"id":"f1"/.test(readFileSync(`${path}.done`, 'utf8'))) failures.push('finish: the second run\'s `.done` is not the second run\'s.')
})

section('treeDigest', () => {
  // What the suite runs against, as bytes: code and data count, prose and caches do not (a README edited meanwhile changes nothing a seed can see).
  const root = join(dir, 'tree')
  mkdirSync(join(root, 'drv', '__pycache__'), { recursive: true })
  writeFileSync(join(root, 'drv', 'a.py'), 'x = 1\n')
  writeFileSync(join(root, 'drv', 'README.md'), 'one')
  const base = treeDigest(root, ['drv'])
  writeFileSync(join(root, 'drv', 'README.md'), 'two')
  writeFileSync(join(root, 'drv', '__pycache__', 'a.pyc'), 'cache')
  if (treeDigest(root, ['drv']) !== base) failures.push('treeDigest: prose or a cache changed the digest of the code.')
  writeFileSync(join(root, 'drv', 'a.py'), 'x = 2\n')
  if (treeDigest(root, ['drv']) === base) failures.push('treeDigest: a changed source file did not change the digest.')
  writeFileSync(join(root, 'drv', 'a.py'), 'x = 1\n')
  writeFileSync(join(root, 'drv', 'b.py'), 'y = 1\n')
  if (treeDigest(root, ['drv']) === base) failures.push('treeDigest: a new source file did not change the digest.')
  rmSync(join(root, 'drv', 'b.py'))
  if (treeDigest(root, ['drv']) !== base) failures.push('treeDigest: the same files gave another digest.')
  if (treeDigest(root, ['drv', 'not-there']) !== treeDigest(root, ['drv'])) failures.push('treeDigest: a path that is not there changed the digest (it is skipped, as a link a suite does not have).')
})

rmSync(dir, { recursive: true, force: true })
if (failures.length) {
  for (const failure of failures) console.error(`::error::${failure}`)
  process.exit(1)
}
console.log(`seed-defects gate refuses 4 kinds of worse report on ${driver}, the floor check refuses 2 more on ${held}, and both accept the unchanged one; check-confidence refuses an entry with no date of its own and ignores the file's; the campaign journal resumes only the run it was begun for (6 ways to differ refused, a damaged tail repaired, a finished run never resumed).`)
