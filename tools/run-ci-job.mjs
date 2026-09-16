/**
 * Run a CI job's checks locally, reading the steps out of `.github/workflows/ci.yml`.
 *
 * # Why this exists
 *
 * `CONTRIBUTING.md` asks for four commands before a pull request. CI runs about eighty, and
 * `DRY-COVERAGE-PLAN.md` records what the gap costs twice:
 *
 * * **F7.** The gate passed on a commit whose CI failed three jobs. One failure was a clippy lint
 *   released after the machine's toolchain; the other two were a step in `invariants` and a step in
 *   `corpus`, neither of which the gate contains.
 * * **F12.** A GitHub job stops at its first failing step, so `invariants` had been reporting one
 *   defect per run while four more sat behind the first — one of them unnoticed for thirteen days.
 *   Every step in that job now carries `if: ${{ !cancelled() }}`, which fixes CI and does nothing
 *   for anybody running checks by hand.
 *
 * The obvious remedy — write the steps out in a script — is a **second record of a fact**, and this
 * codebase's standing argument against those is that they disagree exactly once, after the failure
 * that matters. So this reads `ci.yml` itself. There is nothing here to keep in sync: a step added to
 * the workflow is a step this runs, and a step whose command changes changes here.
 *
 * # What it does not reproduce
 *
 * The runner. `ubuntu-latest` has a toolchain, a `systemd`, a `PATH` and a locale that a developer's
 * machine may not, and F7's first row is exactly that difference. A green run here means the *checks*
 * pass on this machine; it does not mean CI is green, and the summary says so rather than implying
 * otherwise.
 *
 * # Usage
 *
 * ```text
 * node tools/run-ci-job.mjs invariants            # every step, POSIX shell for the shell ones
 * node tools/run-ci-job.mjs invariants --wsl      # ... through `wsl -e bash`, for Windows
 * node tools/run-ci-job.mjs invariants --list     # what it would run, and nothing else
 * node tools/run-ci-job.mjs corpus --only=corpus  # steps whose name matches a substring
 * ```
 *
 * Every step runs whatever the one before it did — the same rule `!cancelled()` gives the workflow,
 * and the reason this is useful at all.
 */

import { readFileSync, writeFileSync, unlinkSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const WORKFLOW = join(ROOT, '.github', 'workflows', 'ci.yml')
const SCRATCH = join(ROOT, '.ci-step.sh')

/**
 * The `run:` steps of one job, in order, each with the name CI shows.
 *
 * Parsed by indentation rather than with a YAML library, because `tools/` takes no dependencies for
 * the same reason the workspace does not. The shape it relies on is the one this file has had since
 * it was written — two spaces per level, `- ` opening a step — and a file that stops having it fails
 * loudly here (no steps found) rather than silently running a subset.
 */
function stepsOf(job) {
  const lines = readFileSync(WORKFLOW, 'utf8').split('\n')
  let i = lines.findIndex((line) => line === `  ${job}:`)
  if (i === -1) throw new Error(`no job named \`${job}\` in ${WORKFLOW}`)

  // The job's body ends where the next top-level job begins.
  let end = lines.length
  for (let k = i + 1; k < lines.length; k++) {
    if (/^ {2}\S/.test(lines[k])) {
      end = k
      break
    }
  }

  const steps = []
  let current = null
  for (let k = i; k < end; k++) {
    const line = lines[k]
    if (/^ {6}- /.test(line)) {
      if (current) steps.push(current)
      current = { name: null, script: null }
      const named = line.match(/^ {6}- name:\s*(.*)$/)
      if (named) current.name = named[1].trim()
    }
    if (!current) continue
    const named = line.match(/^ {8}name:\s*(.*)$/)
    if (named) current.name = named[1].trim()

    const run = line.match(/^ {8}run:\s*(.*)$/)
    if (run) {
      if (run[1].trim() === '|' || run[1].trim() === '|-') {
        // A block: every following line indented past the key, until it is not.
        const body = []
        let j = k + 1
        const indent = (l) => l.length - l.trimStart().length
        const base = lines[j] !== undefined ? indent(lines[j]) : 0
        for (; j < end; j++) {
          if (lines[j].trim() === '') {
            body.push('')
            continue
          }
          if (indent(lines[j]) < base) break
          body.push(lines[j].slice(base))
        }
        current.script = body.join('\n').replace(/\n+$/, '')
        k = j - 1
      } else {
        current.script = run[1]
      }
    }
  }
  if (current) steps.push(current)
  return steps.filter((step) => step.script)
}

/**
 * Whether a step's script only reaches for Node.
 *
 * Node runs on the host on Windows and is frequently absent from the WSL side, so these are spawned
 * directly rather than through the POSIX shell. Classified from the script's own command words, so a
 * step that grows a `grep` stops being a Node step and starts going through the shell — which is the
 * correct direction to be wrong in.
 */
function isNodeOnly(script) {
  const commands = script
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith('#'))
  return commands.length > 0 && commands.every((line) => /^(node|npm|npx) /.test(line))
}

const [, , job, ...flags] = process.argv
if (!job) {
  console.error('usage: node tools/run-ci-job.mjs <job> [--wsl] [--list] [--only=<substring>]')
  process.exit(2)
}
const viaWsl = flags.includes('--wsl')
const listOnly = flags.includes('--list')
const only = flags.find((flag) => flag.startsWith('--only='))?.slice('--only='.length)

let steps = stepsOf(job)
if (only) steps = steps.filter((step) => (step.name ?? '').toLowerCase().includes(only.toLowerCase()))
if (steps.length === 0) {
  console.error(`no runnable steps in \`${job}\`${only ? ` matching \`${only}\`` : ''}.`)
  process.exit(2)
}

if (listOnly) {
  for (const step of steps) {
    console.log(`${isNodeOnly(step.script) ? 'node ' : 'shell'}  ${step.name ?? '(unnamed)'}`)
  }
  process.exit(0)
}

const failed = []
for (const step of steps) {
  const label = step.name ?? '(unnamed)'
  const node = isNodeOnly(step.script)
  let status
  if (node) {
    // Line by line, so a two-command step reports the command that failed rather than the step.
    status = 0
    for (const line of step.script.split('\n').map((l) => l.trim()).filter(Boolean)) {
      const [command, ...args] = line.split(/\s+/)
      const run = spawnSync(command, args, { cwd: ROOT, stdio: 'inherit', shell: true })
      status = run.status ?? 1
      if (status !== 0) break
    }
  } else {
    // `set -e` because a workflow's `run:` block is `bash -e`: a failing line ends the step.
    writeFileSync(SCRATCH, `set -e\n${step.script}\n`)
    const run = viaWsl
      ? spawnSync('wsl', ['-e', 'bash', './.ci-step.sh'], { cwd: ROOT, stdio: 'inherit' })
      : spawnSync('bash', ['./.ci-step.sh'], { cwd: ROOT, stdio: 'inherit' })
    status = run.status ?? 1
  }
  console.log(`${status === 0 ? 'ok  ' : 'FAIL'}  ${label}`)
  if (status !== 0) failed.push(label)
}
try {
  unlinkSync(SCRATCH)
} catch {
  // Never written, because every step was a Node one.
}

console.log('')
if (failed.length > 0) {
  console.log(`${failed.length} of ${steps.length} step(s) failed:`)
  for (const label of failed) console.log(`  ${label}`)
  console.log('')
}
console.log(
  `${steps.length - failed.length} of ${steps.length} step(s) of \`${job}\` pass on this machine. ` +
    `That is the checks, not the runner: a toolchain, a systemd or a locale this machine does not ` +
    `have is F7 in DRY-COVERAGE-PLAN.md, and it is why this is not a claim that CI is green.`
)
process.exit(failed.length > 0 ? 1 : 0)
