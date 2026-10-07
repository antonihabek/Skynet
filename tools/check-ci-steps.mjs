#!/usr/bin/env node
/**
 * Every step of a CI job runs, whatever the step before it did.
 *
 * # Why this exists
 *
 * `node tools/run-ci-job.mjs` says of itself that it "runs every step whatever the one before it
 * did", and it does, because a job that stops at its first red step reports one problem and hides
 * the rest — `CONTRIBUTING.md` records the day four checks were red for up to thirteen days behind
 * another red one. GitHub does not behave that way. A step with no `if:` is skipped once an earlier
 * step has failed, and a skipped step is not red: it is a tick that never ran.
 *
 * On 2026-10-05 the `rust` job's `The daemon, end to end` failed, and four steps after it — the
 * system-manager sandbox, the installer's narration, the setup wizard and the safety MCU build —
 * were skipped; the `abi` job failed at a kernel-CAN step and seventeen after it carried no `if:`,
 * among them the scaffold, the control loop, the camera path and MuJoCo's setup, so the job went
 * on to run the Apollo check without MuJoCo and failed for that, which is a different problem
 * (`DRY-COVERAGE-PLAN.md` F291). The local runner and the hosted one disagreed about the same file.
 *
 * # What it refuses
 *
 * In `.github/workflows/ci.yml`, every step that runs a command (`run:`), after the first such step
 * of its job, carries an `if:` that says `!cancelled()` or `always()`. The first is the job's setup
 * (a toolchain, a checkout's sibling) and nothing after it can run without it; a step that fails
 * *because* its setup did says so, which is the better failure.
 *
 *   node tools/check-ci-steps.mjs            # refuse, naming each step
 *   node tools/check-ci-steps.mjs --write    # add `if: ${{ !cancelled() }}` after each step's `name:`
 *
 * No dependencies: the workflow is read line by line, because a YAML library is a package this
 * repository does not take, and the file's shape — jobs at two spaces, steps at six, keys at eight —
 * is one every workflow in it already has. A step that does not begin with `- name:` is refused
 * rather than guessed at, since an `if:` written after a `run: |` would land inside its script.
 */
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const file = path.join(root, '.github', 'workflows', 'ci.yml')
const write = process.argv.includes('--write')

const GUARD = '        if: ${{ !cancelled() }}'
const lines = fs.readFileSync(file, 'utf8').split('\n')

/** @type {{job: string, name: string, line: number, runs: boolean, guard: boolean, named: boolean}[]} */
const steps = []
let job = null
let inJobs = false
let current = null

for (let index = 0; index < lines.length; index++) {
  const text = lines[index]
  if (/^jobs:\s*$/.test(text)) { inJobs = true; continue }
  if (!inJobs) continue
  const jobMatch = /^  ([A-Za-z0-9_-]+):\s*$/.exec(text)
  if (jobMatch) { job = jobMatch[1]; current = null; continue }
  const stepMatch = /^      - (name|uses|run|if|with|env|id|shell|working-directory|continue-on-error):\s*(.*)$/.exec(text)
  if (stepMatch && job) {
    current = {
      job,
      name: stepMatch[1] === 'name' ? stepMatch[2].replace(/^['"]|['"]$/g, '') : `(step at line ${index + 1})`,
      line: index + 1,
      runs: stepMatch[1] === 'run',
      guard: stepMatch[1] === 'if' && /!cancelled\(\)|always\(\)/.test(stepMatch[2]),
      named: stepMatch[1] === 'name',
    }
    steps.push(current)
    continue
  }
  if (current) {
    const key = /^        ([A-Za-z-]+):\s*(.*)$/.exec(text)
    if (key) {
      if (key[1] === 'run') current.runs = true
      if (key[1] === 'if' && /!cancelled\(\)|always\(\)/.test(key[2])) current.guard = true
    }
  }
}

const problems = []
const insertAfter = []
const seenFirstRun = new Set()
for (const step of steps) {
  if (!step.runs) continue
  if (!seenFirstRun.has(step.job)) { seenFirstRun.add(step.job); continue }
  if (step.guard) continue
  problems.push(step)
  if (step.named) insertAfter.push(step.line - 1)
}

if (write && insertAfter.length > 0) {
  for (const at of insertAfter.sort((a, b) => b - a)) lines.splice(at + 1, 0, GUARD)
  fs.writeFileSync(file, lines.join('\n'))
  console.log(`check-ci-steps: wrote ${insertAfter.length} guard(s) into ${path.relative(root, file)}.`)
  process.exit(problems.length === insertAfter.length ? 0 : 1)
}

if (problems.length === 0) {
  console.log(`CI steps hold: ${steps.filter((step) => step.runs).length} command steps in ${seenFirstRun.size} jobs, each after a job's first carrying an \`if:\` that runs it whatever came before.`)
  process.exit(0)
}

for (const step of problems) {
  console.log(`::error file=.github/workflows/ci.yml,line=${step.line}::job \`${step.job}\`, step "${step.name}" has no \`if: \${{ !cancelled() }}\`, so one red step before it skips it and it reports nothing`)
}
console.log(`\ncheck-ci-steps: ${problems.length} step(s) would be skipped behind an earlier failure. Run: node tools/check-ci-steps.mjs --write`)
process.exit(1)
