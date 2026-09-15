#!/usr/bin/env node
/**
 * Verify the shared documents have not diverged.
 *
 * Three files live in both this repository and the Hub's. §72.1 splits the code for good reasons —
 * licensing history, vendor readability, toolchain overlap, release cadence — and the specification
 * is not one of the things that should be split by that boundary. Two copies drifting apart would
 * mean two teams building against two designs while both believed they were reading the same one.
 *
 * So: a shared document is edited in both repositories or in neither.
 *
 *   node tools/check-mirrors.mjs                      # looks for the Hub beside this checkout
 *   node tools/check-mirrors.mjs ../RobotOverflow     # or say where it is
 *   node tools/check-mirrors.mjs --sync ../RobotOverflow  # copy the Hub's copies over ours
 *   node tools/check-mirrors.mjs --pin  ../RobotOverflow  # copy nothing; pin what already agrees
 *
 * Exits non-zero on divergence, so it can be a pre-commit hook or a CI step in whichever repository
 * has both checked out.
 *
 * # With one repository, which is what CI has
 *
 * The comparison needs both trees, and this script's answer to having one used to be to print
 * that the Hub was not found and exit zero — so the CI job named after this check could not fail, and
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

import { findHubOr } from './hub-path.mjs'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** `[ path in this repository, path in the Hub ]`. */
const MIRRORS = [
  ['docs/skynet-os-architecture.md', 'docs/skynet-os-architecture.md'],
]

// The *code* the two repositories share is checked by `check-contract-mirrors.mjs` rather than
// here, because those files legitimately differ in one way — their import specifiers — and a
// byte comparison would report every one of them as diverged for ever. Run both.

const args = process.argv.slice(2)
const sync = args.includes('--sync')
/**
 * Record agreement without moving a byte.
 *
 * `--sync` conflates two operations that are not the same one: *take the Hub's copies*, and
 * *record that these bytes are agreed*. The workflow this file documents below — "an edit made
 * here first should be copied there by hand, deliberately" — needs the second without the first,
 * and until `--pin` existed the only way to reach it was to run `--sync`, which overwrites **every
 * other** shared document from the other tree. So the documented way to finish an edit made here
 * ended in a command that could silently undo one.
 *
 * The failure is not hypothetical and it is not symmetric: the other tree may hold a shared
 * document mid-edit and uncommitted, which is an ordinary state for a repository somebody is
 * working in. Copying that across as the price of pinning an unrelated file mirrors bytes nobody
 * has committed.
 */
const pinOnly = args.includes('--pin')
const given = args.find(arg => !arg.startsWith('--'))

/**
 * Where the Hub is, when nobody said — `hub-path.mjs`'s list, not this file's.
 *
 * The spellings live in one module because three tools here need them and one of the three had
 * only two of the seven, which took `scale-report.mjs`'s Hub measurement off the air silently.
 * This script's own failure mode is the same one: reporting "not found, nothing to compare" when
 * the checkout is right there under a different capital, and falling back to its pins while
 * passing. A check that cannot run is not a check.
 */
const hub = given ? resolve(given) : findHubOr(HERE).path

const digest = path => createHash('sha256').update(readFileSync(path)).digest('hex')

/**
 * Does this file hold Windows line endings?
 *
 * `.gitattributes` pins the working tree to LF on every platform, for the three reasons stated at
 * the top of that file — the corpus, these mirrors and `versionDigest()` are all decided by exact
 * bytes. A tool that rewrites a file on Windows can undo it silently, and `git add` then normalises
 * on the way in, so the *commit* is right and the **working tree the pin was computed from is not**.
 *
 * That is the failure this guard exists for, and it is a nasty one because every local check passes:
 * the two trees agree, the digest is written, the commit is clean, and CI — which checks out LF —
 * reports the specification as changed without the pin being updated. Refused rather than
 * normalised: silently hashing bytes other than the ones on the disk is how a pin stops being a
 * statement about a file.
 */
function hasCrlf(path) {
  return readFileSync(path).includes(String.fromCharCode(13) + String.fromCharCode(10))
}

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
        + 'the Hub, then re-pin from the two agreeing copies:\n\n'
        + '    node tools/check-mirrors.mjs --pin ../RobotOverflow\n\n'
        + '--pin moves nothing. Use --sync only when the other tree holds the newer copy and you\n'
        + 'want it here. Editing the pins by hand switches this check off.',
    )
  }
  return wrong
}

/**
 * Pin whatever the two repositories currently agree on — document by document.
 *
 * `agreed` is the set that matched this run. A document the two trees disagree about keeps
 * **whatever pin it already had**, which is the conservative answer: the pin's job on a single
 * checkout is to catch a document edited here and not there, and carrying the old value forward
 * leaves that catch armed. Overwriting it would record a disagreement as the agreed state, and
 * dropping it would switch the check off for that file.
 *
 * This used to be all-or-nothing — `if (sync && diverged === 0) writePins()` — and the shape of
 * the bug is worth keeping written down, because it only appears with three mirrors and two
 * repositories in flight. A specification edited here and mirrored across correctly could not be
 * re-pinned, for as long as an *unrelated* shared document was mid-edit in the other tree. The
 * options that leaves are all bad: hand-edit a pin, which this file's own note calls switching the
 * check off; pull the other tree's unfinished work across to make the count zero; or ship with a
 * stale pin and a red CI job. None of them is what the rule is for.
 */
function writePins(agreed) {
  const wrong = [...agreed].filter(ours => hasCrlf(join(HERE, ours)))
  if (wrong.length > 0) {
    console.error(
      `\nRefusing to pin: ${wrong.join(', ')} ${wrong.length === 1 ? 'holds' : 'hold'} CRLF line endings.\n\n`
        + `.gitattributes pins this working tree to LF, and a digest taken over CRLF bytes\n`
        + `describes a file no checkout will ever produce — so it passes here and fails in CI.\n`
        + `Convert the file(s) to LF and run this again; \`git add --renormalize .\` does it for\n`
        + 'a whole tree.',
    )
    process.exit(1)
  }
  const existing = existsSync(PINS)
    ? (JSON.parse(readFileSync(PINS, 'utf8')).digests ?? {})
    : {}
  const digests = {}
  for (const [ours] of MIRRORS) {
    const path = join(HERE, ours)
    if (!existsSync(path)) continue
    if (agreed.has(ours)) {
      digests[ours] = digest(path)
    } else if (existing[ours]) {
      digests[ours] = existing[ours]
      console.log(`kept     ${ours} — pinned at the last agreed bytes; the two copies differ now`)
    }
  }
  writeFileSync(
    PINS,
    `${JSON.stringify({
      note: 'sha256 of each document shared with the Hub (Robot Overflow), pinned so a divergence is caught with '
        + 'one checkout — which is what CI has. Written by tools/check-mirrors.mjs --sync (take '
        + 'their copies, then pin) or --pin (move nothing, pin what already agrees). A document '
        + 'the two trees differ on keeps its last agreed digest rather than being re-pinned or '
        + 'dropped. Never edit by hand: a digest somebody updated to match a local edit is a '
        + 'check that has been switched off.',
      digests,
    }, null, 2)}\n`,
    'utf8',
  )
  console.log('wrote    docs/mirror-digests.json')
}

if (!existsSync(hub)) {
  console.log(`No Hub checkout at ${hub} — the two copies cannot be compared.`)
  const wrong = checkPins()
  if (wrong > 0) process.exit(1)
  console.log('Every shared document matches its pinned digest.')
  process.exit(0)
}

let diverged = 0
let synced = 0
/** The documents the two trees hold identically, after any sync this run performed. */
const agreed = new Set()

for (const [ours, theirs] of MIRRORS) {
  const a = join(HERE, ours)
  const b = join(hub, theirs)

  if (!existsSync(b)) {
    console.log(`MISSING  ${theirs} in the Hub`)
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
    agreed.add(ours)
    continue
  }

  if (sync && !pinOnly) {
    // The Hub is the origin of all three, so it wins a sync. An edit made here first should be
    // copied there by hand, deliberately, rather than by a flag that could silently overwrite it
    // — and then pinned with `--pin`, which is the half of this that moves nothing.
    copyFileSync(b, a)
    console.log(`synced   ${ours}  <- the Hub`)
    synced += 1
    agreed.add(ours)
    continue
  }

  console.log(`DIVERGED ${ours}`)
  console.log(`         here:    ${a}`)
  console.log(`         the Hub: ${b}`)
  diverged += 1
}

if (synced > 0) {
  console.log(`\n${synced} file(s) synced from the Hub.`)
}

// Re-pinned per document rather than per run. Pinning a divergence would record a disagreement as
// the agreed state, which is the one thing a pin must never do — so a diverged document keeps its
// last agreed digest and every other document is re-pinned around it.
if (sync || pinOnly) writePins(agreed)

// Verified even with both trees present: a pin left stale by an edit made in both repositories
// would otherwise survive until the next single-checkout run and fail in CI instead of here.
if (!sync && !pinOnly && checkPins() > 0) process.exit(1)

if (diverged > 0 && !pinOnly) {
  console.error(
    `\n${diverged} shared document(s) have diverged.\n` +
      'A shared document is edited in both repositories or in neither. Either copy the newer\n' +
      'version across by hand, or run with --sync to take the Hub\'s copies.',
  )
  process.exit(1)
}

if (diverged > 0) {
  // `--pin` left them alone on purpose, and says so rather than reporting a clean run.
  console.log(
    `\n${diverged} shared document(s) still differ and kept their last agreed pin. `
      + 'Nothing was copied.',
  )
  process.exit(0)
}

console.log('\nAll shared documents match.')
