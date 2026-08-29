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
 * has both checked out. It exits **zero** when BotMarket is simply not present, because the OS
 * repository has to build for somebody who has only cloned this one — a vendor writing a driver
 * should never be told their tree is broken because a private repository is missing.
 */

import { createHash } from 'node:crypto'
import { copyFileSync, existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** `[ path in this repository, path in BotMarket ]`. */
const MIRRORS = [
  ['docs/skynet-os-architecture.md', 'docs/skynet-os-architecture.md'],
  ['docs/skynet-architecture.md', 'docs/skynet-architecture.md'],
  ['docs/skynet-build-plan.md', 'skynet-build-plan.md'],
]

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
const CANDIDATES = ['BotMarket', 'botmarket', 'Botmarket', 'botMarket']

function findHub() {
  for (const name of CANDIDATES) {
    const candidate = resolve(join(HERE, '..', name))
    if (existsSync(candidate)) return candidate
  }
  return resolve(join(HERE, '..', 'BotMarket'))
}

const hub = given ? resolve(given) : findHub()

if (!existsSync(hub)) {
  console.log(`BotMarket not found at ${hub} — nothing to compare.`)
  console.log('This is not a failure: the OS repository stands on its own.')
  process.exit(0)
}

const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex')

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

if (diverged > 0) {
  console.error(
    `\n${diverged} shared document(s) have diverged.\n` +
      'A shared document is edited in both repositories or in neither. Either copy the newer\n' +
      'version across by hand, or run with --sync to take BotMarket\'s copies.',
  )
  process.exit(1)
}

console.log('\nAll shared documents match.')
