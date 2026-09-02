#!/usr/bin/env node
/**
 * Verify the shared documents have not diverged.
 *
 * Three files live in both this repository and BotMarket. §72.1 splits the code for good reasons —
 * licensing history, vendor readability, toolchain overlap, release cadence — and the specification
 * is not one of the things that should be split by that boundary. Two copies drifting apart would
 * mean two teams building against two designs while both believed they were reading the same one.
 *
 * So: a shared document is edited in both repositories or in neither.
 *
 *   node tools/check-mirrors.mjs                 # looks for BotMarket beside this checkout
 *   node tools/check-mirrors.mjs ../BotMarket    # or say where it is
 *   node tools/check-mirrors.mjs --sync ../BotMarket   # copy BotMarket's copies over ours
 *
 * Exits non-zero on divergence, so it can be a pre-commit hook or a CI step in whichever repository
 * has both checked out.
 *
 * # With one repository, which is what CI has
 *
 * The comparison needs both trees, and this script's answer to having one used to be to print
 * "BotMarket not found" and exit zero — so the CI job named after this check could not fail, and
 * a specification edited here and not there would ship.
 *
 * So each document's digest is pinned in `docs/mirror-digests.json`, written by `--sync` from the
 * copies the two repositories agreed on. With one checkout the script verifies the documents
 * against those pins, which catches exactly the thing the rule forbids: a shared document edited
 * here without being edited there. A fresh clone matches its pins and passes, so a vendor who has
 * only cloned this repository is still never told their tree is broken because a private one is
 * missing.
 */

import { createHash } from 'node:crypto'
import { copyFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** `[ path in this repository, path in BotMarket ]`. */
const MIRRORS = [
  ['docs/skynet-os-architecture.md', 'docs/skynet-os-architecture.md'],
  ['docs/skynet-architecture.md', 'docs/skynet-architecture.md'],
  ['docs/skynet-build-plan.md', 'skynet-build-plan.md'],
]

// The *code* the two repositories share is checked by `check-contract-mirrors.mjs` rather than
// here, because those files legitimately differ in one way — their import specifiers — and a
// byte comparison would report every one of them as diverged for ever. Run both.

const args = process.argv.slice(2)
const sync = args.includes('--sync')
const given = args.find(arg => !arg.startsWith('--'))

/**
 * Where BotMarket is, when nobody said.
 *
 * Several spellings, because the repository is `botmarket` on GitHub, `BotMarket` in the
 * specification's prose, and whatever `git clone` produced on disk. A case-sensitive filesystem
 * makes those three different directories, and this script's whole failure mode is reporting "not
 * found, nothing to compare" when the checkout is right there under a different capital — a check
 * that cannot run is not a check, which is the sentence this file already says about CI.
 */
// The Hub was renamed from BotMarket to Robot Overflow. Both spellings are here because a checkout
// made before the rename is still a valid checkout — and because a tool that can only find the old
// name prints "not found", falls back to checking its own pins, and goes on passing on a machine
// that has both trees. That is the degraded mode written for CI doing its job silently where the
// real comparison was available.
const CANDIDATES = [
  'Robot Overflow', 'robot-overflow', 'RobotOverflow', 'BotMarket', 'botmarket', 'Botmarket',
  'botMarket',
]

function findHub() {
  for (const name of CANDIDATES) {
    const candidate = resolve(join(HERE, '..', name))
    if (existsSync(candidate)) return candidate
  }
  return resolve(join(HERE, '..', 'BotMarket'))
}

const hub = given ? resolve(given) : findHub()

const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex')

/**
 * The pinned digests of the shared documents.
 *
 * Written by `--sync`, from the copies the two repositories agreed on at that moment, and checked
 * on every run — including the run with one checkout, which is the one CI performs.
 *
 * A pin is not a second source of truth. It records that these bytes were agreed, so that editing
 * one copy afterwards is a visible act rather than a silent divergence discovered by whoever next
 * has both trees in front of them.
 */
const PINS = join(HERE, 'docs/mirror-digests.json')

function checkPins() {
  if (!existsSync(PINS)) {
    console.error('\ndocs/mirror-digests.json is missing. Run with --sync to write it.')
    return 1
  }
  const pinned = JSON.parse(readFileSync(PINS, 'utf8')).digests ?? {}
  let wrong = 0
  for (const [ours] of MIRRORS) {
    const path = join(HERE, ours)
    if (!existsSync(path)) {
      console.error(`MISSING  ${ours} here`)
      wrong += 1
      continue
    }
    const current = digest(path)
    if (pinned[ours] === current) continue
    console.error(
      pinned[ours]
        ? `CHANGED  ${ours} — this document no longer matches its pinned digest.`
        : `UNPINNED ${ours} — this document has no pinned digest.`,
    )
    wrong += 1
  }
  if (wrong > 0) {
    console.error(
      `\n${wrong} shared document(s) changed here without the pin being updated.\n`
        + 'A shared document is edited in both repositories or in neither. Make the same edit in\n'
        + 'BotMarket, then re-pin from the two agreeing copies:\n\n'
        + '    node tools/check-mirrors.mjs --sync ../BotMarket\n\n'
        + 'Editing the pins by hand switches this check off.',
    )
  }
  return wrong
}

/** Pin whatever the two repositories currently agree on. */
function writePins() {
  const digests = {}
  for (const [ours] of MIRRORS) {
    const path = join(HERE, ours)
    if (existsSync(path)) digests[ours] = digest(path)
  }
  writeFileSync(
    PINS,
    `${JSON.stringify({
      note: 'sha256 of each document shared with BotMarket, pinned so a divergence is caught with '
        + 'one checkout — which is what CI has. Written by tools/check-mirrors.mjs --sync. Never '
        + 'edit by hand: a digest somebody updated to match a local edit is a check that has been '
        + 'switched off.',
      digests,
    }, null, 2)}\n`,
    'utf8',
  )
  console.log('wrote    docs/mirror-digests.json')
}

if (!existsSync(hub)) {
  console.log(`BotMarket not found at ${hub} — the two copies cannot be compared.`)
  const wrong = checkPins()
  if (wrong > 0) process.exit(1)
  console.log('Every shared document matches its pinned digest.')
  process.exit(0)
}

let diverged = 0
let synced = 0

for (const [ours, theirs] of MIRRORS) {
  const a = join(HERE, ours)
  const b = join(hub, theirs)

  if (!existsSync(b)) {
    console.log(`MISSING  ${theirs} in BotMarket`)
    diverged += 1
    continue
  }
  if (!existsSync(a)) {
    console.log(`MISSING  ${ours} here`)
    diverged += 1
    continue
  }

  if (digest(a) === digest(b)) {
    console.log(`match    ${ours}`)
    continue
  }

  if (sync) {
    // BotMarket is the origin of all three, so it wins a sync. An edit made here first should be
    // copied there by hand, deliberately, rather than by a flag that could silently overwrite it.
    copyFileSync(b, a)
    console.log(`synced   ${ours}  <- BotMarket`)
    synced += 1
    continue
  }

  console.log(`DIVERGED ${ours}`)
  console.log(`         here:      ${a}`)
  console.log(`         BotMarket: ${b}`)
  diverged += 1
}

if (synced > 0) {
  console.log(`\n${synced} file(s) synced from BotMarket.`)
}

// Re-pinned only when the two copies agree, which after a sync they do. Pinning a divergence
// would record a disagreement as the agreed state, which is the one thing a pin must never do.
if (sync && diverged === 0) writePins()

// Verified even with both trees present: a pin left stale by an edit made in both repositories
// would otherwise survive until the next single-checkout run and fail in CI instead of here.
if (!sync && diverged === 0 && checkPins() > 0) process.exit(1)

if (diverged > 0) {
  console.error(
    `\n${diverged} shared document(s) have diverged.\n` +
      'A shared document is edited in both repositories or in neither. Either copy the newer\n' +
      'version across by hand, or run with --sync to take BotMarket\'s copies.',
  )
  process.exit(1)
}

console.log('\nAll shared documents match.')
