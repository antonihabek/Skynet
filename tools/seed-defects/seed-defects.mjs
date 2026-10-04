#!/usr/bin/env node
/**
 * Defect seeding — how many realistic defects does a driver's dry suite actually notice?
 * (DRY-COVERAGE-PLAN.md item 12.C.3.)
 *
 * The plan refuses to put a percentage on "works on the robot": that number would say the residual
 * risk is small and known, and it is neither. What *can* be measured is a **detection rate**: take a
 * driver, seed it with a defect of a kind that has really happened to a driver in this repository (a
 * sign, a unit, a byte order, a latch that outlives its session, a stop that only fires once…), run
 * the strongest suite that can run here, and count how often the suite goes red. A seed that leaves
 * the suite green is a **survivor**: a defect of that kind the suite would not have caught.
 *
 * ```
 * node tools/seed-defects/seed-defects.mjs --driver ur_rtde          # one driver
 * node tools/seed-defects/seed-defects.mjs --all --jobs 4            # everything runnable
 * node tools/seed-defects/seed-defects.mjs --check                   # static: every seed still applies
 * node tools/seed-defects/seed-defects.mjs --all --report            # write reference/reports/seed-defects.json
 * node tools/seed-defects/seed-defects.mjs --driver ur_rtde-rs --rung 1 --survivors-of 4 --report
 *                                                                     # a higher rung re-runs only what rung 4 missed
 * node tools/seed-defects/seed-defects.mjs --reclassify               # work the report out again after triaging survivors
 * node tools/seed-defects/seed-defects.mjs --gate --baseline FILE     # a fresh report against the committed one (the nightly)
 * ```
 *
 * A survivor is either a gap (write the test), an **equivalent mutant** — nothing observable differs, so no test could see it;
 * the seed says why in `equivalent`, is left out of the rate, and must still survive — or a defect this rung cannot observe,
 * which stays a survivor and says why in `note`. Environment: `SEED_PYTHON` is the interpreter, and a suite with `python: NAME`
 * runs under `SEED_PYTHON_NAME` (the venvs that carry `requests` or a vendor's client).
 *
 * # What makes the number mean something
 *
 * * **The baseline runs first.** A suite that is red before any defect is seeded kills every seed.
 * * **A seed applies exactly once or the run stops.** `find` must occur exactly once in `file`; a
 *   seed that silently matches nothing would be a survivor for the wrong reason, and a seed that
 *   matches twice mutates something its author did not read. Sources change, so this is loud.
 * * **A defect known to be undetectable is in every driver's set and must survive.** `expect:
 *   "survive"` seeds are the metric's control: a rate that cannot fall below 100% measures the
 *   patcher, not the suite. If one is *killed* it is reported as a mis-labelled seed.
 * * **Seeds are authored, not generated.** Each names the finding that makes it realistic (`class`,
 *   one of the classes in `classes.json`, and `why`). A survivor is a finding to write a test for;
 *   it is not a score to defend.
 * * **Suites run on copies, and the repository is never patched.** Python drivers are copied to a temporary
 *   tree per seed (with the directories they import linked in). A Rust driver runs in a copy of the cargo
 *   workspace — a few megabytes, because the workspace has no third-party dependencies — of which each worker
 *   keeps one for the whole campaign, so cargo's fingerprints stay warm, the seeds run in parallel, and the
 *   counterparts a suite runs against (`links`) are snapshotted when the campaign starts, so an edit made to
 *   one meanwhile cannot change what a later seed was measured against. An interrupted run leaves temporary
 *   directories and nothing else.
 *
 * What the rate measures is the *suite*: it says a class of defect would have been caught by the
 * counterpart the suite runs against (rung 4 for the hermetic suites here). It does not say the
 * counterpart is right, that no other defect is present, or anything about a robot.
 */
import { spawn } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { HERE, REPO, REPORT, die, digestOfSeeds, loadSeeds, readReport, suites } from './common.mjs'

const GATED = join(HERE, 'gated.json')

// ── arguments ────────────────────────────────────────────────────────────────
const args = process.argv.slice(2)
const flag = name => args.includes(`--${name}`)
const value = name => { const i = args.indexOf(`--${name}`); return i >= 0 ? args[i + 1] : undefined }
const jobs = Number(value('jobs') ?? 4)
/** Each driver's unpatched ABI outcomes, taken by the baseline run and compared against by every seed. */
const baselines = {}
const only = value('driver')
/** The current driver's workspace copies (Rust suites), removed when the driver's campaign ends. */
let workspaces = []
const onlySeed = value('seed')

/** The text a seed applies to, and the number of times `find` occurs in it (it must be one). */
function sourceOf(driver, seed) {
  const suite = suites[driver]
  const path = join(REPO, suite.dir, seed.file)
  if (!existsSync(path)) return { path, error: `${suite.dir}/${seed.file} does not exist` }
  const text = readFileSync(path, 'utf8').replace(/\r\n/g, '\n')
  const occurrences = text.split(seed.find).length - 1
  return { path, text, occurrences }
}

/** The environment variables a suite needs and this process does not have: the checkout of a vendor's SDK a compiled driver links. */
function unmetRequirements(suite) {
  return (suite.requires ?? []).filter(name => !process.env[name])
}

function checkStatic(all) {
  let bad = 0
  let total = 0
  for (const [driver, seeds] of Object.entries(all)) {
    if (only && only !== driver) continue
    for (const seed of seeds) {
      total++
      const src = sourceOf(driver, seed)
      if (src.error) { console.error(`  STALE  ${seed.id}: ${src.error}`); bad++ }
      else if (src.occurrences !== 1) {
        console.error(`  STALE  ${seed.id}: \`find\` occurs ${src.occurrences} times in ${suites[driver].dir}/${seed.file}; it must occur exactly once`)
        bad++
      }
    }
    if (!seeds.some(seed => seed.expect === 'survive' && !seed.equivalent)) {
      console.error(`  NO CONTROL  ${driver}: no seed expects to survive, so its rate cannot fall — add a defect the suite cannot see`)
      bad++
    }
  }
  return { bad, total }
}

/**
 * The floor a driver has earned (`gated.json`): its committed report must still say it, about this seed set. This is the
 * "≥99% becomes an invariants gate once met" of item 12.C.3, and it is static — it reads the report, it runs nothing — so
 * it can sit in the per-push job while the campaign that refreshes the report is nightly.
 */
function checkFloor(all) {
  const out = { problems: [], drivers: 0, floor: 0.99, rung: 4 }
  if (!existsSync(GATED)) return out
  const gated = JSON.parse(readFileSync(GATED, 'utf8'))
  out.floor = gated.floor
  out.rung = gated.rung
  const report = readReport()
  for (const driver of gated.drivers) {
    if (only && only !== driver) continue
    out.drivers++
    if (!all[driver]) { out.problems.push(`${driver} is gated and has no seeds.`); continue }
    const entry = report.drivers?.[driver]?.[String(gated.rung)]
    if (!entry) { out.problems.push(`${driver} is gated at rung ${gated.rung} and the report has no run of it: node tools/seed-defects/seed-defects.mjs --driver ${driver} --report`); continue }
    if (entry.seedsDigest !== digestOfSeeds({ [driver]: all[driver] })) { out.problems.push(`${driver}: the report measured a different seed set from the one in the tree, so the floor says nothing about these seeds. Run it again with --report.`); continue }
    if (entry.rate === null || entry.rate < gated.floor) out.problems.push(`${driver}: detection is ${entry.rate === null ? 'unmeasured' : (100 * entry.rate).toFixed(1) + '%'} (${entry.killed}/${entry.seeds}) and this driver has met ${(100 * gated.floor).toFixed(0)}%. Survivors: ${entry.survivors.map(s => s.id).join(', ') || 'none'}.`)
    if (entry.controlsSurvived !== entry.controls || entry.mislabelledControls.length) out.problems.push(`${driver}: its controls did not all survive, so its rate no longer measures the suite.`)
  }
  return out
}

// ── running a suite ──────────────────────────────────────────────────────────
function run(command, cwd, env, timeoutSeconds) {
  return new Promise(resolvePromise => {
    const started = Date.now()
    const child = spawn('bash', ['-c', command], { cwd, env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'], detached: true })
    let output = ''
    child.stdout.on('data', chunk => { output += chunk; if (output.length > 200000) output = output.slice(-100000) })
    child.stderr.on('data', chunk => { output += chunk; if (output.length > 200000) output = output.slice(-100000) })
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      try { process.kill(-child.pid, 'SIGKILL') } catch { /* already gone */ }
    }, timeoutSeconds * 1000)
    child.on('close', code => {
      clearTimeout(timer)
      resolvePromise({ code: timedOut ? 124 : code, timedOut, output, seconds: (Date.now() - started) / 1000 })
    })
  })
}

function envFor(suite, root) {
  const env = {}
  for (const [key, template] of Object.entries(suite.env ?? {})) env[key] = template.replaceAll('{root}', root)
  env.PYTHONDONTWRITEBYTECODE = '1'
  // A workspace copy builds into a directory of its own, so two workers never share cargo's lock or each other's
  // half-built patched crate, and nothing is written to the checkout's `target`.
  if (suite.workspace) env.CARGO_TARGET_DIR = join(root, '.target')
  return env
}

/** A temporary tree shaped like the checkout: the driver copied, what it imports linked. */
function makeTree(driver) {
  const suite = suites[driver]
  const root = mkdtempSync(join(tmpdir(), `seed-${driver}-`))
  const target = join(root, suite.dir)
  mkdirSync(dirname(target), { recursive: true })
  cpSync(join(REPO, suite.dir), target, {
    recursive: true,
    filter: source => !/(__pycache__|\.pyc$|node_modules|[\\/]target([\\/]|$))/.test(source),
  })
  for (const link of suite.links ?? []) {
    const linkPath = join(root, link)
    mkdirSync(dirname(linkPath), { recursive: true })
    if (!existsSync(linkPath)) symlinkSync(join(REPO, link), linkPath)
  }
  return root
}

function apply(text, seed) {
  const at = text.indexOf(seed.find)
  return text.slice(0, at) + seed.replace + text.slice(at + seed.find.length)
}

async function seedInTree(driver, suite, seed) {
  const root = makeTree(driver)
  try {
    const path = join(root, suite.dir, seed.file)
    const original = readFileSync(path, 'utf8')
    const crlf = original.includes('\r\n')
    const text = original.replace(/\r\n/g, '\n')
    if (text.split(seed.find).length - 1 !== 1) return { verdict: 'stale', detail: 'find does not occur exactly once' }
    const patched = apply(text, seed)
    writeFileSync(path, crlf ? patched.replace(/\n/g, '\r\n') : patched)
    return await runSuite(suite, root, baselines[driver])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

/** The interpreter a suite runs under. Drivers that import `requests` or a vendor client run under a venv. */
function pythonFor(suite) {
  const named = suite.python ? process.env[`SEED_PYTHON_${suite.python.toUpperCase()}`] : undefined
  return named ?? process.env.SEED_PYTHON ?? 'python3'
}

/**
 * `skynet driver check` through the real ABI, against the driver's own simulated controller: the
 * second half of the strongest suite that runs here. Its exit code says guest-tier failures; the
 * results document says every test's outcome, so a defect that fails a test above guest, or turns
 * a pass into a skip, is seen too — compared with the outcomes of the unpatched driver.
 */
async function runAbi(suite, root, resultsPath) {
  const abi = suite.abi
  const python = pythonFor(suite)
  const pythonPath = [join(root, 'bindings', 'python'), join(root, suite.dir), ...(abi.extraPath ?? []).map(p => join(root, p))].join(':')
  const env = { ...envFor(suite, root), SKYNET_PYTHON: python, SKYNET_PYTHON_DRIVER: abi.module, PYTHONPATH: pythonPath, ...(abi.env ?? {}) }
  const config = abi.config ? ` --config '${abi.config}'` : ''
  const command = `${REPO}/target/release/skynet driver check ${REPO}/drivers/python-host/libskynet_python_host.so${config} --results ${resultsPath}`
  const result = await run(command, join(root, suite.dir), env, suite.timeout ?? 300)
  let outcomes = null
  try {
    outcomes = Object.fromEntries(JSON.parse(readFileSync(resultsPath, 'utf8')).results.map(r => [r.test_id, r.outcome]))
  } catch { /* the driver failed to describe itself: no results document */ }
  return { ...result, outcomes }
}

async function runSuite(suite, root, baseline) {
  const cwd = join(root, suite.dir)
  const env = envFor(suite, root)
  let seconds = 0
  for (const template of suite.run) {
    const command = template.replaceAll('{python}', pythonFor(suite)).replaceAll('{repo}', REPO).replaceAll('{root}', root)
    const result = await run(command, cwd, env, suite.timeout ?? 300)
    seconds += result.seconds
    if (result.code !== 0) return killed(result, suite, seconds, command)
  }
  if (suite.abi) {
    const result = await runAbi(suite, root, join(root, 'abi-results.json'))
    seconds += result.seconds
    if (result.code !== 0 || !result.outcomes) return killed(result, suite, seconds, 'skynet driver check')
    if (!baseline) return { verdict: 'survived', detail: '', seconds, outcomes: result.outcomes }
    const changed = Object.entries(baseline).filter(([id, outcome]) => result.outcomes[id] !== outcome)
    if (changed.length) {
      const [id, outcome] = changed[0]
      return { verdict: 'killed', detail: `driver check: ${id} was ${outcome}, now ${result.outcomes[id] ?? 'absent'}`, seconds, command: 'skynet driver check' }
    }
  }
  return { verdict: 'survived', detail: '', seconds }
}

/**
 * A patch that does not compile is not a defect: the compiler refused it, and no suite saw anything. rustc says `error[E…]` and
 * `could not compile`; a C++ compiler says `file.cpp:12:3: error:` — a position, then the word, which a suite's own failure line
 * ("FAIL …", "error 14") never has.
 */
const NOT_A_PROGRAM = /could not compile|error\[E\d+\]|error: aborting due to|\.(?:cpp|hpp|cc|h):\d+:\d+: error:/

function killed(result, suite, seconds, command) {
  // A step the shell could not start (127: not found, 126: not executable) ran nothing, so no suite noticed anything. The SCHUNK campaign counted 24 seeds as killed because `target/release/skynet` had been
  // deleted under it (the Windows build cache, cleared to make room): the step after a green selftest failed with "No such file or directory", and each of those seeds had in fact passed everything that ran.
  if ((result.code === 127 || result.code === 126) && !result.timedOut) {
    const first = result.output.split('\n').find(line => /not found|No such file|cannot execute|Permission denied/i.test(line)) ?? `the step exited ${result.code}`
    return { verdict: 'invalid', detail: `${first.trim().slice(0, 140)} — the step did not run: nothing was measured`, seconds, command, tail: result.output.trim().split('\n').slice(-25).join('\n') }
  }
  if (NOT_A_PROGRAM.test(result.output) && !result.timedOut) {
    const first = result.output.split('\n').find(line => /^error|: error:/.test(line)) ?? 'the seeded source does not compile'
    return { verdict: 'invalid', detail: first.trim().slice(0, 160), seconds, command, tail: result.output.trim().split('\n').slice(-25).join('\n') }
  }
  const lines = result.output.trim().split('\n').filter(Boolean)
  // The first check a suite says failed, when it says so in the form `  FAIL  <what>`; otherwise the last line that sounds like a failure
  // (a build tool's own "Error 1" comes last and says nothing about which check).
  const firstFailed = lines.find(line => /^\s*FAIL\b/.test(line))
  const why = result.timedOut ? `timed out after ${suite.timeout ?? 300}s (a hang is a detection)` : (firstFailed ?? lines.filter(line => /FAIL|Error|error|assert|✗|failed/i.test(line)).slice(-1)[0] ?? lines.slice(-1)[0] ?? '').trim().slice(0, 160)
  const tail = result.output.trim().split('\n').slice(-25).join('\n')
  return { verdict: 'killed', detail: why, seconds, command, tail }
}

// ── Rust: a copy of the workspace per worker ─────────────────────────────────
/**
 * What a cargo workspace needs to build and test one driver crate: every member's manifest must exist, and `skynet-cli`
 * (which the EtherCAT check builds) embeds `drivers/models.json`, `packaging/install.sh` and the C header. All source,
 * a few megabytes, and no third-party dependency — which is why this can be copied when the plan's first design could not.
 */
const WORKSPACE_BASE = ['Cargo.toml', 'Cargo.lock', 'crates', 'bindings', 'packaging/install.sh', 'drivers/models.json', 'drivers/cia402-rs', 'drivers/franka-rs', 'drivers/canopen-rs', 'drivers/ur_rtde-rs', 'drivers/ethercat-rs']

/** One worker's copy: the workspace, and the counterparts the suite runs against, as they are now. */
function makeWorkspace(driver, index) {
  const suite = suites[driver]
  const root = mkdtempSync(join(tmpdir(), `seed-ws-${driver}-${index}-`))
  for (const path of [...WORKSPACE_BASE, ...(suite.links ?? [])]) {
    const from = join(REPO, path)
    if (!existsSync(from)) die(`${driver}: the workspace copy needs ${path}, which the checkout does not have.`)
    const to = join(root, path)
    mkdirSync(dirname(to), { recursive: true })
    cpSync(from, to, { recursive: true, filter: source => !/(__pycache__|\.pyc$|node_modules|[\\/]target([\\/]|$))/.test(source) })
  }
  return root
}

/** Patch the worker's copy, run the suite, and put the file back: the copy outlives the seed, so cargo stays warm. */
async function seedInWorkspace(root, driver, suite, seed) {
  const path = join(root, suite.dir, seed.file)
  const raw = readFileSync(path, 'utf8')
  const crlf = raw.includes('\r\n')
  const text = raw.replace(/\r\n/g, '\n')
  if (text.split(seed.find).length - 1 !== 1) return { verdict: 'stale', detail: 'find does not occur exactly once' }
  const patched = apply(text, seed)
  writeFileSync(path, crlf ? patched.replace(/\n/g, '\r\n') : patched)
  try {
    return await runSuite(suite, root, baselines[driver])
  } finally {
    writeFileSync(path, raw)
  }
}

/** `work(item, worker)`: a worker keeps its number, so the state a worker owns (its workspace copy) is only ever used by it. */
async function pool(items, width, work) {
  const results = new Array(items.length)
  let next = 0
  async function worker(id) {
    for (;;) {
      const index = next++
      if (index >= items.length) return
      results[index] = await work(items[index], id)
    }
  }
  await Promise.all(Array.from({ length: Math.max(1, width) }, (_, id) => worker(id)))
  return results
}

// ── main ─────────────────────────────────────────────────────────────────────
async function main() {
  const all = loadSeeds()

  if (flag('check')) {
    const { bad, total } = checkStatic(all)
    if (bad) die(`${bad} problem(s) in ${total} seeds. A seed that no longer applies measures nothing: fix or delete it.`)
    const held = checkFloor(all)
    if (held.problems.length) {
      for (const problem of held.problems) console.error(`  FLOOR  ${problem}`)
      die(`${held.problems.length} driver(s) fell below the detection floor they had met. Write the test a survivor shows is missing; do not lower the floor.`)
    }
    console.log(`${total} seeds across ${Object.keys(all).length} drivers each apply exactly once, and every driver has a control that must survive; ${held.drivers} driver${held.drivers === 1 ? '' : 's'} hold the ${(100 * held.floor).toFixed(0)}% floor at rung ${held.rung}.`)
    return
  }

  if (flag('gate')) {
    // The nightly's verdict. It runs the whole campaign with `--report` over a checkout whose report is the committed one, then this,
    // with a copy of the committed report taken *before* the run. A suite that got worse — a test weakened, a check deleted, a seed a
    // suite used to notice and no longer does — shows here as a drop, and a drop fails: the rate is a floor to hold, and a seed's
    // survival is a finding to write a test for, not a number to argue with.
    const baselinePath = value('baseline')
    if (!baselinePath || !existsSync(baselinePath)) die('--gate needs --baseline FILE, a copy of the committed report taken before the run to compare the fresh one with.')
    if (!existsSync(REPORT)) die(`--gate: no fresh report at ${REPORT}. Run the campaign with --report first.`)
    const before = JSON.parse(readFileSync(baselinePath, 'utf8'))
    const fresh = JSON.parse(readFileSync(REPORT, 'utf8'))
    const rung = String(value('rung') ?? 4)
    const problems = []
    let compared = 0
    for (const [driver, byRung] of Object.entries(before.drivers ?? {})) {
      if ((only && only !== driver) || !byRung[rung]) continue
      // A driver whose suite needs a vendor's SDK checkout the machine does not have was not run by `--all` (see below), so there is
      // nothing fresh to compare its committed run with. Named explicitly it is compared, and fails if it was not run.
      if (!only && unmetRequirements(suites[driver]).length) continue
      const then = byRung[rung]
      const now = fresh.drivers?.[driver]?.[rung]
      if (!now) { problems.push(`${driver}: the committed report has a rung ${rung} run and the fresh one has none, so the campaign did not reach it.`); continue }
      compared++
      if (!all[driver]) { problems.push(`${driver}: has a report and no seeds any more.`); continue }
      if (now.seedsDigest !== digestOfSeeds({ [driver]: all[driver] })) problems.push(`${driver}: the fresh report measured a different seed set from the one in the tree (a run of an older checkout?).`)
      if (now.mislabelledControls.length) problems.push(`${driver}: a control or an equivalent mutant was noticed (${now.mislabelledControls.join(', ')}); the seed is mislabelled or the patcher is broken.`)
      if (now.invalidSeeds.length) problems.push(`${driver}: seeds that no longer compile: ${now.invalidSeeds.join(', ')}.`)
      if (now.controlsSurvived !== now.controls) problems.push(`${driver}: only ${now.controlsSurvived} of ${now.controls} controls survived, so the rate no longer measures the suite.`)
      const lost = Object.entries(then.verdicts ?? {}).filter(([id, verdict]) => verdict === 'killed' && now.verdicts?.[id] === 'survived').map(([id]) => id)
      if (lost.length) problems.push(`${driver}: no longer notices ${lost.join(', ')}: a test that used to catch ${lost.length === 1 ? 'that defect' : 'those defects'} has been weakened or removed, or the source moved under it.`)
      if (then.rate !== null && now.rate !== null && now.rate < then.rate) {
        const fewer = Object.keys(now.verdicts ?? {}).filter(id => !(id in (then.verdicts ?? {})))
        problems.push(`${driver}: detection fell from ${(100 * then.rate).toFixed(1)}% to ${(100 * now.rate).toFixed(1)}% (${now.killed}/${now.seeds}, was ${then.killed}/${then.seeds})${fewer.length ? `; ${fewer.length} seed${fewer.length === 1 ? ' is' : 's are'} new since, and a new survivor is a test to write` : ''}.`)
      }
    }
    for (const problem of problems) console.error(`  GATE  ${problem}`)
    if (problems.length) die(`${problems.length} regression${problems.length === 1 ? '' : 's'} in defect detection at rung ${rung}. Write the test the survivor shows is missing, or restore the one that went, and run the campaign again.`)
    console.log(`defect detection held at rung ${rung}: ${compared} driver${compared === 1 ? '' : 's'} no worse than the committed report.`)
    return
  }

  if (flag('reclassify')) {
    // Work the report out again from each seed's verdict and what has since been decided about it (a survivor triaged as
    // equivalent, a note added). Nothing is run, and a run of seeds that have since changed is left as it was.
    if (!existsSync(REPORT)) die(`no report at ${REPORT} to reclassify.`)
    const merged = JSON.parse(readFileSync(REPORT, 'utf8'))
    for (const [driver, byRung] of Object.entries(merged.drivers)) {
      if ((only && only !== driver) || !all[driver]) continue
      for (const [rung, entry] of Object.entries(byRung)) {
        if (entry.seedsDigest !== digestOfSeeds({ [driver]: all[driver] })) { console.error(`  ${driver} at rung ${rung}: the seeds changed after this run, so it is left as it was. Run it again.`); continue }
        // Nothing is run here, so the entry keeps the day it was actually measured.
        byRung[rung] = { ...summarise(driver, all[driver], { rung: entry.rung, run: entry.suite, abi: null }, entry.verdicts, entry.killedBelow), ...(entry.measured ? { measured: entry.measured } : {}) }
        console.log(describeSummary(driver, rung, byRung[rung]))
      }
    }
    merged.generated = new Date().toISOString().slice(0, 10)
    writeFileSync(REPORT, JSON.stringify(merged, null, 2) + '\n')
    return
  }

  const drivers = Object.keys(all).filter(driver => !only || driver === only)
  if (!drivers.length) die(only ? `no seeds for "${only}".` : 'no seeds.')
  const { bad } = checkStatic(Object.fromEntries(drivers.map(driver => [driver, all[driver]])))
  if (bad) die('stale seeds; run with --check for the list.')

  const report = { generated: new Date().toISOString().slice(0, 10), drivers: {} }
  for (const driver of drivers) {
    const rung = Number(value('rung') ?? 4)
    // `--all` runs what this machine can: a suite that links a vendor's SDK says which environment variable names the checkout
    // (`requires`), and without it the driver is skipped, out loud. Named with `--driver` it runs and fails if the SDK is not there.
    const unmet = unmetRequirements(suites[driver])
    if (!only && unmet.length) { process.stderr.write(`\n${driver}: skipped, ${unmet.join(' and ')} not set (the suite links the vendor's SDK; name the checkout to run it).\n`); continue }
    const base = suites[driver]
    // Rung 4 is the hermetic suite. A higher rung is the same driver against software this repository
    // did not write, which needs a substrate to be started first (and stopped, whatever happens).
    const substrate = rung === 4 ? null : base.substrates?.[String(rung)]
    if (rung !== 4 && !substrate) { process.stderr.write(`\n${driver}: no rung ${rung} substrate is defined for seeding; skipped.\n`); continue }
    const suite = substrate ? { ...base, ...substrate, rung } : { ...base, rung: 4 }
    let seeds = all[driver].filter(seed => !onlySeed || seed.id === onlySeed)
    // A higher rung runs the lower rung's suite too, so a seed the lower rung noticed is noticed here as well. Only what
    // it missed is worth a substrate's minutes: `--survivors-of 4` reads the report's rung-4 run and re-runs its survivors.
    let carried = 0
    let prior
    const below = value('survivors-of')
    if (below) {
      prior = existsSync(REPORT) ? JSON.parse(readFileSync(REPORT, 'utf8')).drivers?.[driver]?.[below] : undefined
      if (!prior) die(`--survivors-of ${below}: the report has no rung ${below} run for ${driver}. Run that first with --report.`)
      if (prior.seedsDigest !== digestOfSeeds({ [driver]: all[driver] })) die(`--survivors-of ${below}: the seeds changed after that run, so its survivors are not this set's. Run it again.`)
      const missed = new Set(prior.survivors.map(survivor => survivor.id))
      seeds = seeds.filter(seed => missed.has(seed.id) || seed.expect === 'survive')
      carried = prior.killed
    }
    process.stderr.write(`\n${driver} — rung ${suite.rung}, ${seeds.length} seeds${carried ? ` (the ${carried} that rung ${below} noticed are not re-run)` : ''}, suite: ${[...suite.run, ...(suite.abi ? ['skynet driver check'] : [])].join(' && ')}\n`)

    if (suite.setup) {
      const setup = await run(suite.setup.replaceAll('{repo}', REPO), REPO, {}, suite.setupTimeout ?? 300)
      if (setup.code !== 0) die(`${driver}: the rung ${rung} substrate would not start:\n${setup.output.slice(-800)}`)
    }
    try {
      // The baseline: the suite must pass before any defect is put in, and pass *every time*. A suite that is red one run
      // in five would "kill" one seed in five for reasons that are its own, and a rate measured with it is a rate of the
      // flake (a Fourier check failed four runs in five behind a single retry, F187). So it runs `baselineRuns` times in a
      // row on each copy — three for a hermetic suite, once for a substrate that is minutes a run and started just for this —
      // and one red among them stops the campaign, naming how many greens came first.
      // A workspace suite keeps one copy per worker for the whole campaign; a tree suite makes one per run.
      const width = suite.serial ? 1 : Math.max(1, Math.min(jobs, seeds.length))
      workspaces = suite.workspace ? Array.from({ length: width }, (_, index) => makeWorkspace(driver, index)) : []
      const runs = suite.baselineRuns ?? (suite.setup ? 1 : 3)
      const once = async (root) => root ? await runSuite(suite, root) : await (async () => { const tree = makeTree(driver); try { return await runSuite(suite, tree) } finally { rmSync(tree, { recursive: true, force: true }) } })()
      const attempt = async (root) => {
        let first
        for (let n = 1; n <= runs; n++) {
          const result = await once(root)
          if (result.verdict !== 'survived') return { ...result, detail: `${result.detail} (run ${n} of ${runs}${n > 1 ? `, after ${n - 1} green` : ''})` }
          // A suite that reports each test's outcome must report the same ones every time.
          if (first?.outcomes && JSON.stringify(result.outcomes) !== JSON.stringify(first.outcomes)) return { verdict: 'flaky', detail: `the outcomes of \`skynet driver check\` differ between run 1 and run ${n}`, seconds: result.seconds }
          first ??= result
        }
        return first
      }
      // Every worker's copy runs the baseline (in parallel), which is also what warms its cargo build before any seed is timed.
      const firsts = await Promise.all((workspaces.length ? workspaces : [null]).map(root => attempt(root)))
      const baseline = firsts.find(result => result.verdict !== 'survived') ?? firsts[0]
      if (baseline.verdict !== 'survived') die(`${driver}'s suite is not green before any defect is seeded (${baseline.detail}); every seed would be "killed" for that reason. Fix the suite first.
--- the suite's last lines (${baseline.command}):
${baseline.tail ?? ''}`)
      if (baseline.outcomes) baselines[driver] = baseline.outcomes
      process.stderr.write(`  baseline green ${runs} time${runs === 1 ? '' : 's'} in a row (${baseline.seconds.toFixed(1)}s a run)\n`)

    const results = await pool(seeds, width, async (seed, worker) => {
      const result = suite.workspace ? await seedInWorkspace(workspaces[worker], driver, suite, seed) : await seedInTree(driver, suite, seed)
      const mark = result.verdict === 'killed' ? 'killed  ' : result.verdict === 'survived' ? 'SURVIVED' : result.verdict === 'invalid' ? 'INVALID ' : 'STALE   '
      process.stderr.write(`  ${mark} ${seed.id.padEnd(28)} ${seed.class.padEnd(22)} ${seed.expect === 'survive' ? '(control) ' : ''}${result.detail}\n`)
      return { ...seed, ...result }
    })

    // What every seed came to at this rung. A higher rung re-ran only what the lower one missed, so the rest are noticed by construction.
    const verdicts = {}
    if (prior) for (const [id, verdict] of Object.entries(prior.verdicts)) if (verdict === 'killed') verdicts[id] = 'killed'
    for (const r of results) verdicts[r.id] = r.verdict
    const summary = summarise(driver, all[driver], suite, verdicts, carried)
    report.drivers[driver] ??= {}
    report.drivers[driver][String(rung)] = summary
    process.stderr.write(`  ${describeSummary(driver, rung, summary)}\n`)
    // Written as each driver finishes, not at the end: campaigns run for hours and two may run at once, and a report
    // that is only read and written in the last millisecond of each is one they cannot trample.
    if (flag('report') && !onlySeed) writeReport({ [driver]: { [String(rung)]: summary } })
    } finally {
      for (const root of workspaces) rmSync(root, { recursive: true, force: true })
      workspaces = []
      if (suite.teardown) await run(suite.teardown.replaceAll('{repo}', REPO), REPO, {}, 120)
    }
  }

  const worst = Object.values(report.drivers).some(byRung => Object.values(byRung).some(d => d.mislabelledControls.length || d.invalidSeeds.length))
  if (worst) process.exit(1)
}

/**
 * Merge what one driver's run came to into the report, keeping every other driver's and every other rung's.
 *
 * Each entry carries the day *it* was measured. `generated` is only the day the file was last written, and
 * `drivers/confidence.json` reads the per-entry date: with one file-wide date, re-running one driver moved
 * every other driver's "measured" date with it, and a record said fourteen drivers had been measured on a day
 * they had not (DRY-COVERAGE-PLAN.md F223).
 */
function writeReport(byDriver) {
  mkdirSync(dirname(REPORT), { recursive: true })
  let merged = { drivers: {} }
  if (existsSync(REPORT)) merged = JSON.parse(readFileSync(REPORT, 'utf8'))
  const today = new Date().toISOString().slice(0, 10)
  merged.generated = today
  delete merged.seedsDigest
  for (const [driver, byRung] of Object.entries(byDriver)) {
    const stamped = Object.fromEntries(Object.entries(byRung).map(([rung, entry]) => [rung, { ...entry, measured: today }]))
    merged.drivers[driver] = { ...(merged.drivers[driver] ?? {}), ...stamped }
  }
  writeFileSync(REPORT, JSON.stringify(merged, null, 2) + '\n')
  console.log(`wrote ${REPORT} (${Object.keys(byDriver).join(', ')})`)
}

/**
 * What a run came to, from the seeds and each one's verdict alone — so the same numbers can be worked out again, without
 * running anything, when a survivor has since been triaged (`--reclassify`) or a seed's note has changed.
 */
function summarise(driver, seeds, suite, verdicts, killedBelow) {
  const results = seeds.map(seed => ({ ...seed, verdict: verdicts[seed.id] }))
  // A seed the compiler refused measured nothing: it is neither killed nor a survivor, and it is listed so it gets fixed.
  const invalid = results.filter(r => r.verdict === 'invalid')
  const counted = results.filter(r => r.expect === 'kill' && r.verdict !== 'invalid' && r.verdict !== undefined)
  const controls = results.filter(r => r.expect === 'survive' && !r.equivalent && r.verdict !== undefined)
  const equivalents = results.filter(r => r.equivalent && r.verdict !== undefined)
  const killed = counted.filter(r => r.verdict === 'killed')
  const survivors = counted.filter(r => r.verdict === 'survived')
  const mislabelled = [...controls, ...equivalents].filter(r => r.verdict === 'killed')
  return {
    rung: suite.rung, suite: [...suite.run, ...(suite.abi ? ['skynet driver check'] : [])], seedsDigest: digestOfSeeds({ [driver]: seeds }),
    seeds: counted.length, killed: killed.length, survived: survivors.length, killedBelow: killedBelow || undefined,
    rate: counted.length ? Number((killed.length / counted.length).toFixed(3)) : null,
    controls: controls.length, controlsSurvived: controls.length - controls.filter(r => r.verdict === 'killed').length,
    byClass: tally(counted),
    survivors: survivors.map(r => ({ id: r.id, class: r.class, why: r.why, ...(r.note ? { note: r.note } : {}) })),
    equivalents: equivalents.map(r => ({ id: r.id, class: r.class, reason: r.equivalent, survived: r.verdict === 'survived' })),
    mislabelledControls: mislabelled.map(r => r.id),
    invalidSeeds: invalid.map(r => r.id),
    verdicts,
  }
}

function describeSummary(driver, rung, s) {
  const equivalent = s.equivalents.length ? `, ${s.equivalents.length} equivalent mutant${s.equivalents.length === 1 ? '' : 's'} left out` : ''
  return `${driver} at rung ${rung}: ${s.killed} of ${s.seeds} seeded defects noticed (${s.rate === null ? 'n/a' : (100 * s.rate).toFixed(0) + '%'}), ${s.survived} survived${equivalent}; ${s.controlsSurvived} of ${s.controls} controls survived as they must${s.mislabelledControls.length ? `; MISLABELLED (killed, so neither a control nor equivalent): ${s.mislabelledControls.join(', ')}` : ''}${s.invalidSeeds.length ? `; ${s.invalidSeeds.length} SEEDS DID NOT COMPILE (excluded): ${s.invalidSeeds.join(', ')}` : ''}`
}

function tally(results) {
  const byClass = {}
  for (const r of results) {
    byClass[r.class] ??= { seeds: 0, killed: 0 }
    byClass[r.class].seeds++
    if (r.verdict === 'killed') byClass[r.class].killed++
  }
  return byClass
}

main().catch(error => { console.error(error); process.exit(2) })
