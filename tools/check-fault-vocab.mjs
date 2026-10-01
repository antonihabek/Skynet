#!/usr/bin/env node
/**
 * The fault-vocabulary table (`drivers/fault-vocabulary.json`, DRY-COVERAGE-PLAN.md item 12.C.6) says what each driver makes of the codes
 * its robot's maker defines, and `tools/fault-vocab/contract.py` (and, for the Rust ports, a test that reads the same file) is what holds
 * a driver to it. This holds the *table* to what can be checked without running a driver. It cannot say the table is right about a
 * maker's codes; it can refuse a table that is not one the contract can hold anybody to, and a driver nobody has asked.
 *
 * # What it refuses
 *
 * 1. A driver that commands a physical robot (every row of `drivers/confidence.json`) with no entry — an entry says what it covers, or says
 *    in words (`pending`, at least 40 characters) why it covers nothing yet. Silence is not an answer. And an entry for a driver that is
 *    not there.
 * 2. A vocabulary the contract cannot hold: no codes, a class outside the four, a code with no name of the maker's, an `unmapped` class
 *    that lets an unlisted code read as healthy (an alarm channel escalates to `fault` or `estop`; only a `warning` channel may stop at
 *    `degraded`), fewer than three probes — or none, with a stated reason — a probe that is a code the table lists, or zero (§24's healthy),
 *    a `source` that does not say what was measured against the maker's software and what was only read, an unknown `fault_code` kind.
 * 3. A Python driver whose subject file is missing or names another driver, whose Makefile has no `faultvocab` target, that no workflow
 *    runs, that has no `faultvocab` suite in the confidence matrix claiming only layers the contract reaches (4 and 5, never 9 or 10), that
 *    has no seeded defect of class `vendor-code-class` (a contract nobody has watched kill anything), or whose seeded suite does not run
 *    the contract.
 *    A `keeps` list (the motions a body moved by motions and not by joint commands still answers in a failsafe) is a list of names with a
 *    reason, and has a fault class to stay available in.
 * 4. A Rust port whose test is missing, or does not read `fault-vocabulary.json`.
 *
 *   node tools/check-fault-vocab.mjs             # check
 *   node tools/check-fault-vocab.mjs --selftest  # the check, shown refusing each kind of worse table
 */
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = path => readFileSync(join(ROOT, path), 'utf8').replace(/\r\n/g, '\n')
const exists = path => existsSync(join(ROOT, path))

const CLASSES = ['ok', 'degraded', 'fault', 'estop']
const ALARM_ESCALATIONS = ['fault', 'estop']
const WARNING_ESCALATIONS = ['degraded', 'fault', 'estop']
const FAULT_CODES = ['verbatim', 'derived', 'absent']
const KINDS = ['alarm', 'warning']
/** The layers of `drivers/confidence.json` the contract reaches: refusal semantics, and the stop and the fault path. */
const REACHED = [4, 5]

/** The targets a Makefile defines, by the plain `name:` lines make itself reads. */
function makeTargets(dir, ctx) {
  const path = `drivers/${dir}/Makefile`
  if (!ctx.exists(path)) return new Set()
  const targets = new Set()
  for (const line of ctx.read(path).split('\n')) {
    const match = /^([A-Za-z0-9_.-]+)\s*:(?!=)/.exec(line)
    if (match) targets.add(match[1])
  }
  return targets
}

/** Every refusal, for `doc` against the tree `ctx` describes. Pure: the self-test feeds it worse tables. */
export function check(doc, ctx) {
  const failures = []
  const fail = message => failures.push(message)
  const rows = Object.keys(ctx.confidence.drivers)

  for (const driver of rows) {
    if (!(driver in doc.drivers)) fail(`${driver}: commands a robot (a row of drivers/confidence.json) and has no entry in drivers/fault-vocabulary.json. Add one, or say in \`pending\` why it covers nothing yet.`)
  }
  for (const driver of Object.keys(doc.drivers)) {
    if (!rows.includes(driver)) fail(`${driver}: has an entry in drivers/fault-vocabulary.json and no row in drivers/confidence.json, so it is not a driver that commands a robot.`)
  }

  for (const [driver, entry] of Object.entries(doc.drivers)) {
    const hasTable = entry.vocabularies && Object.keys(entry.vocabularies).length > 0
    const reusesTable = typeof entry.table_of === 'string'
    if (!hasTable && !reusesTable) {
      if (typeof entry.pending !== 'string' || entry.pending.length < 40) fail(`${driver}: covers no vocabulary and says nothing about why (\`pending\` must say, in at least 40 characters).`)
      continue
    }
    if (reusesTable) {
      const twin = doc.drivers[entry.table_of]
      if (!twin || !twin.vocabularies) fail(`${driver}: shares the table of ${entry.table_of}, which has none.`)
    }
    for (const [name, v] of Object.entries(entry.vocabularies ?? {})) {
      const at = `${driver} ${name}`
      const codes = v.codes ?? {}
      if (Object.keys(codes).length === 0) fail(`${at}: lists no codes.`)
      if (!KINDS.includes(v.kind)) fail(`${at}: \`kind\` is ${JSON.stringify(v.kind)}, not one of ${KINDS.join(' or ')}.`)
      for (const [code, row] of Object.entries(codes)) {
        if (!CLASSES.includes(row.class)) fail(`${at} ${code}: class ${JSON.stringify(row.class)} is not one of ${CLASSES.join(', ')}.`)
        if (typeof row.name !== 'string' || row.name.trim() === '') fail(`${at} ${code}: carries no name — the maker's own word for it is how a person finds it in their documentation.`)
        if (!/^-?\d+$/.test(code)) fail(`${at} ${code}: a code is a whole number.`)
      }
      const allowed = v.kind === 'warning' ? WARNING_ESCALATIONS : ALARM_ESCALATIONS
      if (!allowed.includes(v.unmapped)) fail(`${at}: an unlisted code escalates to ${JSON.stringify(v.unmapped)}, which would let it read as healthy or as less than the channel allows (${allowed.join(' or ')}).`)
      if (typeof v.unmapped_because !== 'string' || v.unmapped_because.length < 40) fail(`${at}: says nothing about why an unlisted code escalates as it does.`)
      const probes = v.probes ?? []
      if (v.no_probes) {
        if (probes.length !== 0 || String(v.no_probes).length < 40) fail(`${at}: no probes, which needs an empty list and a reason of at least 40 characters.`)
      } else if (probes.length < 3) {
        fail(`${at}: ${probes.length} probe(s); at least three codes outside the table, so one lucky value is not the evidence.`)
      }
      for (const probe of probes) {
        if (String(probe) in codes) fail(`${at}: probe ${probe} is a code the table lists, so it probes nothing.`)
        if (Number(probe) === 0) fail(`${at}: probe 0 is §24's healthy, and no driver could carry it as a fault.`)
      }
      if (!FAULT_CODES.includes(v.fault_code)) fail(`${at}: \`fault_code\` is ${JSON.stringify(v.fault_code)}, not one of ${FAULT_CODES.join(', ')}.`)
      if (v.keeps !== undefined) {
        if (!Array.isArray(v.keeps) || v.keeps.length === 0 || v.keeps.some(motion => typeof motion !== 'string' || motion === '')) fail(`${at}: \`keeps\` is not a list of motion names.`)
        if (typeof v.keeps_because !== 'string' || v.keeps_because.length < 40) fail(`${at}: \`keeps\` says which motions stay available in a failsafe and not why (\`keeps_because\`, at least 40 characters).`)
        if (!Object.values(codes).some(row => row.class === 'fault')) fail(`${at}: \`keeps\` names motions that stay available in a failsafe, and the table has no fault class for them to stay available in.`)
      }
      if (typeof v.source !== 'string' || !/measured|read/i.test(v.source) || v.source.length < 60) fail(`${at}: \`source\` does not say where the table comes from and what was measured against the maker's software and what was only read.`)
      if (typeof v.channel !== 'string' || v.channel.length < 10) fail(`${at}: \`channel\` does not say where the driver reads the code from.`)
    }
    for (const gap of entry.not_covered ?? []) {
      if (!gap.what || !gap.why || gap.why.length < 40) fail(`${driver}: a vocabulary left uncovered needs a \`what\` and a reason of at least 40 characters (${gap.what ?? 'unnamed'}).`)
    }

    // The subject, for a Python driver; the test, for a Rust one.
    const dir = driver
    if (entry.subject) {
      if (!ctx.exists(entry.subject)) fail(`${driver}: its subject ${entry.subject} does not exist.`)
      else {
        const text = ctx.read(entry.subject)
        const named = /^\s*driver\s*=\s*"([^"]+)"/m.exec(text)
        if (!named || named[1] !== driver) fail(`${driver}: ${entry.subject} says it is the subject of ${named ? named[1] : 'nothing'}.`)
      }
      if (!makeTargets(dir, ctx).has('faultvocab')) fail(`${driver}: drivers/${dir}/Makefile has no \`faultvocab\` target, so the contract is not something a person can run.`)
      if (!ctx.ci.includes(`make -C drivers/${dir} faultvocab`)) fail(`${driver}: no workflow runs \`make -C drivers/${dir} faultvocab\`, so a table that nobody enforces is a memory.`)
      const suite = ctx.confidence.drivers[driver]?.suites?.faultvocab
      if (!suite) fail(`${driver}: drivers/confidence.json has no \`faultvocab\` suite for it.`)
      else {
        if (suite.run !== `make -C drivers/${dir} faultvocab`) fail(`${driver}: its \`faultvocab\` suite runs ${JSON.stringify(suite.run)}, not the make target.`)
        for (const layer of suite.reaches ?? []) if (!REACHED.includes(layer)) fail(`${driver}: its \`faultvocab\` suite claims layer ${layer}; the contract reaches ${REACHED.join(' and ')} and never the hardware or the site.`)
        if (!(suite.reaches ?? []).length) fail(`${driver}: its \`faultvocab\` suite reaches no layer.`)
      }
      const seeds = ctx.seeds[driver] ?? []
      if (!seeds.some(seed => seed.class === 'vendor-code-class')) fail(`${driver}: no seeded defect of class vendor-code-class, so the contract has never been shown to kill a driver that translates a code wrongly.`)
      const suites = ctx.suites[driver]
      if (!suites || !(suites.run ?? []).some(command => command.includes('fault-vocab/contract.py'))) fail(`${driver}: its seeded suite (tools/seed-defects/suites.json) does not run the contract, so a seed the contract alone would catch is counted as surviving.`)
    }
    if (entry.rust_test) {
      const { file, name } = entry.rust_test
      if (!ctx.exists(file)) fail(`${driver}: its test file ${file} does not exist.`)
      else {
        const text = ctx.read(file)
        if (!new RegExp(`fn\\s+${name}\\b`).test(text)) fail(`${driver}: ${file} has no test \`${name}\`.`)
        if (!text.includes('fault-vocabulary.json')) fail(`${driver}: ${file} does not read drivers/fault-vocabulary.json, so the port can be given a table of its own.`)
      }
      const seeds = ctx.seeds[driver] ?? []
      if (!seeds.some(seed => seed.class === 'vendor-code-class')) fail(`${driver}: no seeded defect of class vendor-code-class, so its test has never been shown to kill a port that translates a code wrongly.`)
    }
    if (hasTable && !entry.subject && !entry.rust_test) fail(`${driver}: covers vocabularies and names neither a Python subject nor a Rust test.`)
  }
  return failures
}

/** The tree as it is. */
export function realContext() {
  const seeds = {}
  const suites = JSON.parse(read('tools/seed-defects/suites.json')).suites
  for (const name of Object.keys(suites)) {
    const path = `tools/seed-defects/seeds/${name}.json`
    if (exists(path)) seeds[name] = JSON.parse(read(path)).seeds
  }
  return {
    exists,
    read,
    confidence: JSON.parse(read('drivers/confidence.json')),
    ci: ['ci.yml', 'faults.yml', 'substrates.yml'].filter(name => exists(`.github/workflows/${name}`)).map(name => read(`.github/workflows/${name}`)).join('\n'),
    seeds,
    suites,
  }
}

/** The check shown refusing each kind of worse table, with no driver run. Each variant must fail, and for its reason. */
function selftest() {
  const ctx = realContext()
  const good = JSON.parse(read('drivers/fault-vocabulary.json'))
  const clone = () => JSON.parse(JSON.stringify(good))
  const bound = Object.entries(good.drivers).find(([, e]) => e.subject && e.vocabularies)[0]
  const firstVocabulary = doc => Object.values(doc.drivers[bound].vocabularies)[0]
  const variants = [
    ['a driver with no entry', doc => { delete doc.drivers.mavlink }, 'has no entry'],
    ['an entry for a driver that is not there', doc => { doc.drivers.nothing = { pending: 'x'.repeat(50) } }, 'no row in drivers/confidence.json'],
    ['a driver that says nothing about why it is uncovered', doc => { doc.drivers.mavlink = { pending: 'later' } }, 'says nothing about why'],
    ['a class outside the four', doc => { Object.values(firstVocabulary(doc).codes)[0].class = 'bad' }, 'is not one of ok, degraded, fault, estop'],
    ['a code with no name', doc => { Object.values(firstVocabulary(doc).codes)[0].name = '' }, 'carries no name'],
    ['an alarm whose unlisted code reads as degraded', doc => { const v = firstVocabulary(doc); v.kind = 'alarm'; v.unmapped = 'degraded' }, 'would let it read as healthy'],
    ['an unlisted code escalating to ok', doc => { firstVocabulary(doc).unmapped = 'ok' }, 'would let it read as healthy'],
    ['two probes', doc => { firstVocabulary(doc).probes = [901, 902]; delete firstVocabulary(doc).no_probes }, 'at least three codes outside the table'],
    ['a probe that is a listed code', doc => { const v = firstVocabulary(doc); v.probes = [Object.keys(v.codes)[0], 901, 902, 903] }, 'probes nothing'],
    ['a probe of zero', doc => { firstVocabulary(doc).probes = [0, 901, 902, 903] }, "§24's healthy"],
    ['a source that says nothing was checked', doc => { firstVocabulary(doc).source = 'the maker documents these codes in a manual somewhere on their site' }, 'does not say where the table comes from'],
    ['a fault word of a kind nobody knows', doc => { firstVocabulary(doc).fault_code = 'whatever' }, 'not one of verbatim'],
    ['a subject that is another driver', doc => { doc.drivers[bound].subject = 'drivers/xarm/faultvocab.py'; if (bound === 'xarm') doc.drivers[bound].subject = 'drivers/franka/faultvocab.py' }, 'says it is the subject of'],
    ['a subject that does not exist', doc => { doc.drivers[bound].subject = 'drivers/nowhere/faultvocab.py' }, 'does not exist'],
    ['a Rust test that does not exist', doc => { doc.drivers['ur_rtde-rs'].rust_test.name = 'no_such_test' }, 'has no test'],
    ['motions that stay available with no reason', doc => { const v = firstVocabulary(doc); v.keeps = ['land']; delete v.keeps_because }, 'and not why'],
    ['motions that stay available and no fault class for them', doc => { const v = firstVocabulary(doc); v.keeps = ['land']; v.keeps_because = 'x'.repeat(50); for (const row of Object.values(v.codes)) if (row.class === 'fault') row.class = 'estop' }, 'no fault class for them'],
    ['an empty list of motions that stay available', doc => { const v = firstVocabulary(doc); v.keeps = []; v.keeps_because = 'x'.repeat(50) }, 'not a list of motion names'],
    ['an uncovered vocabulary with no reason', doc => { (doc.drivers[bound].not_covered ??= []).push({ what: 'something', why: 'no' }) }, 'needs a `what` and a reason'],
  ]
  const baseline = check(good, ctx)
  let bad = 0
  console.log(`the table as committed: ${baseline.length === 0 ? 'passes' : `FAILS (${baseline.length})`}`)
  if (baseline.length) {
    for (const message of baseline) console.log(`  ${message}`)
    bad += 1
  }
  for (const [what, mutate, phrase] of variants) {
    const doc = clone()
    mutate(doc)
    const failures = check(doc, ctx)
    const right = failures.some(message => message.includes(phrase))
    console.log(`  ${right ? 'refused' : 'NOT REFUSED'}  ${what}${right ? '' : ` — wanted a complaint containing \`${phrase}\`; got ${failures.length ? failures.slice(0, 2).join(' | ') : 'none'}`}`)
    if (!right) bad += 1
  }
  // A confidence row that claims the hardware is refused; a seeded suite that does not run the contract is refused.
  const brokenConfidence = JSON.parse(JSON.stringify(ctx.confidence))
  brokenConfidence.drivers[bound].suites.faultvocab.reaches = [4, 5, 9]
  const claimed = check(good, { ...ctx, confidence: brokenConfidence }).some(message => message.includes('never the hardware'))
  console.log(`  ${claimed ? 'refused' : 'NOT REFUSED'}  a faultvocab suite that claims layer 9`)
  if (!claimed) bad += 1
  const brokenSuites = JSON.parse(JSON.stringify(ctx.suites))
  brokenSuites[bound].run = brokenSuites[bound].run.filter(command => !command.includes('fault-vocab'))
  const unrun = check(good, { ...ctx, suites: brokenSuites }).some(message => message.includes('does not run the contract'))
  console.log(`  ${unrun ? 'refused' : 'NOT REFUSED'}  a seeded suite that does not run the contract`)
  if (!unrun) bad += 1
  console.log(bad ? `\n${bad} variant(s) not refused` : `\nevery worse table refused`)
  return bad ? 1 : 0
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  if (process.argv.includes('--selftest')) process.exit(selftest())
  const failures = check(JSON.parse(read('drivers/fault-vocabulary.json')), realContext())
  if (failures.length) {
    for (const message of failures) console.error(`fault-vocabulary: ${message}`)
    console.error(`\nfault-vocabulary: ${failures.length} problem(s). A table the contract cannot hold anybody to measures nothing.`)
    process.exit(1)
  }
  const doc = JSON.parse(read('drivers/fault-vocabulary.json'))
  const covered = Object.values(doc.drivers).filter(entry => entry.vocabularies || entry.table_of).length
  console.log(`fault-vocabulary: ${covered} of ${Object.keys(doc.drivers).length} drivers covered; the rest say why not.`)
}
