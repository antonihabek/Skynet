#!/usr/bin/env node
/**
 * Verify the Hub's copies of the reference contracts have not drifted.
 *
 * §31.4 makes the normative artifact *"the specification plus a corpus of golden vectors that every
 * edition must reproduce exactly"*, and the corpus does hold the Rust edition and the TypeScript
 * oracle to each other on every run. It cannot see the Hub, which keeps its **own** copies of those
 * modules under `lib/skynet/os/` so a Next.js application can import them without a package.
 *
 * That gap is not theoretical. When this check was written, `lib/skynet/os/safety.ts` was six
 * sessions behind — no pose guards, no `withGrants` — and `embodiment.ts` had neither collision
 * shapes nor frame-binding validation nor `capabilities` on the descriptor. The Hub's own test
 * suite passed the whole time, because it was asserting the behaviour of the kernel it had.
 *
 * A robot and a Hub disagreeing about `evaluateCycle` is the exact failure §31 exists to prevent,
 * and "we remember to copy the file across" is not a mechanism. This is.
 *
 *   node tools/check-contract-mirrors.mjs                 # looks for BotMarket beside this checkout
 *   node tools/check-contract-mirrors.mjs ../BotMarket    # or say where it is
 *   node tools/check-contract-mirrors.mjs --sync ../BotMarket
 *
 * # The one legitimate difference
 *
 * Import specifiers. The reference is standalone and uses explicit `.ts` extensions and a local
 * `types.ts`; the Hub resolves through its bundler and takes `SafetyEnvelope` from its own object
 * model. So the comparison normalises exactly those two rewrites and nothing else — a check that
 * normalised more would be a check that could not see a divergence in the code.
 *
 * Exits **zero** when BotMarket is not present, for the same reason `check-mirrors.mjs` does: a
 * vendor who has cloned only this repository should never be told their tree is broken.
 */

import { createHash } from 'node:crypto'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Modules the Hub keeps its own copy of.
 *
 * `calibration` (§45) and `watchdog` (§46) are deliberately **not** here yet. Both are contracts the
 * Hub will need for §32.1's `/skynet/os/devices/[id]` — "active and unavailable guards with the
 * missing input named" is precisely what calibration produces — and that page waits on a table that
 * does not exist. Mirroring them now would put two modules nothing imports into an application, and
 * the honest place to record that they are coming is here rather than in somebody's memory.
 *
 * Add a name to this list and run `--sync`; nothing else is needed.
 */
const MIRRORS = [
  'canonical', 'conformance', 'digest', 'embodiment', 'kinematics', 'package', 'readiness', 'safety',
]

const REFERENCE = 'reference/typescript'
const HUB = 'lib/skynet/os'

const args = process.argv.slice(2)
const sync = args.includes('--sync')
const given = args.find(arg => !arg.startsWith('--'))

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

/** The reference's spelling of an import, rewritten to the Hub's. */
function toHub(source) {
  return lf(source)
    .replace(/from '\.\/types\.ts'/g, "from '../types'")
    .replace(/from '\.\/([a-z-]+)\.ts'/g, "from './$1'")
}

/**
 * One line ending, so a checkout convention cannot present as a contract divergence.
 *
 * This is not tidiness. Both repositories are cloned on Windows with `core.autocrlf=true`, and the
 * first `git checkout` after a merge re-materialised the Hub's copies with CRLF while this
 * repository's reference kept LF. From that moment the comparison below reported all six mirrored
 * modules as diverged **in every line**, and the Hub's eight digest assertions all failed — on two
 * trees that were character for character identical.
 *
 * A check that always fails is a check somebody deletes, which would have left the drift this tool
 * exists to catch unwatched again. `.gitattributes` in both repositories now pins these files to
 * LF so the situation does not recur; this normalisation is the belt to that pair of braces, and it
 * costs nothing, because a line ending is never a semantic difference in TypeScript.
 */
function lf(source) {
  return source.split('\r\n').join('\n')
}

let diverged = 0
let synced = 0
let missing = 0

for (const name of MIRRORS) {
  const ours = join(HERE, REFERENCE, `${name}.ts`)
  const theirs = join(hub, HUB, `${name}.ts`)

  if (!existsSync(ours)) {
    console.log(`SKIP     ${name} — no reference copy`)
    continue
  }
  const expected = toHub(readFileSync(ours, 'utf8'))

  if (!existsSync(theirs)) {
    if (sync) {
      writeFileSync(theirs, expected, 'utf8')
      console.log(`created  ${HUB}/${name}.ts`)
      synced += 1
    } else {
      console.log(`MISSING  ${HUB}/${name}.ts — the Hub has no copy of this contract`)
      missing += 1
    }
    continue
  }

  if (lf(readFileSync(theirs, 'utf8')) === expected) {
    console.log(`match    ${name}`)
    continue
  }

  if (sync) {
    writeFileSync(theirs, expected, 'utf8')
    console.log(`synced   ${HUB}/${name}.ts  <- ${REFERENCE}/${name}.ts`)
    synced += 1
    continue
  }

  // A count rather than the diff: a thousand lines of TypeScript in a CI log is noise, and the
  // useful output is "these two answer differently, go and look".
  //
  // Counted as a multiset difference rather than line by line at the same index. The naive version
  // reports every line after an inserted one as changed, which turned a two-line drift in
  // `conformance.ts` into "247 lines" the first time this ran — a number that is wrong in the
  // direction that makes somebody ignore it.
  const count = lines => {
    const seen = new Map()
    for (const line of lines) seen.set(line, (seen.get(line) ?? 0) + 1)
    return seen
  }
  const mine = count(lf(readFileSync(theirs, 'utf8')).split('\n'))
  const yours = count(expected.split('\n'))
  let differing = 0
  for (const [line, n] of mine) differing += Math.max(0, n - (yours.get(line) ?? 0))
  for (const [line, n] of yours) differing += Math.max(0, n - (mine.get(line) ?? 0))
  console.log(`DIVERGED ${name} — ${differing} line(s) differ`)
  diverged += 1
}

/**
 * The digests, written into the Hub beside the copies.
 *
 * This half catches a different failure from the comparison above, and both are needed. Running
 * this script catches *the reference having moved ahead*, and it only runs where both repositories
 * are checked out. The digest file catches the Hub's copy being edited in place — which is what
 * a well-meaning fix to a type error in a Next.js application looks like — and it is checked by the
 * Hub's own suite, on a machine that has never heard of this repository.
 */
if (sync) {
  const digests = {}
  for (const name of MIRRORS) {
    const ours = join(HERE, REFERENCE, `${name}.ts`)
    if (!existsSync(ours)) continue
    digests[name] = createHash('sha256').update(toHub(readFileSync(ours, 'utf8'))).digest('hex')
  }
  writeFileSync(
    join(hub, HUB, 'mirror-digests.json'),
    `${JSON.stringify({
      note: 'sha256 of each reference contract, with its imports rewritten for this repository. '
        + 'Written by tools/check-contract-mirrors.mjs --sync in the Skynet OS repository; '
        + 'checked by lib/__tests__/skynet-os-mirrors.test.ts. Never edit by hand — a digest '
        + 'somebody updated to match a local edit is a check that has been switched off.',
      digests,
    }, null, 2)}\n`,
    'utf8',
  )
  console.log(`wrote    ${HUB}/mirror-digests.json`)
}

if (synced > 0) console.log(`\n${synced} file(s) synced into BotMarket.`)

if (diverged > 0 || missing > 0) {
  console.error(
    `\n${diverged} contract(s) have diverged and ${missing} are missing from the Hub.\n` +
      'The reference is the oracle the corpus holds the Rust edition to; the Hub keeps its own copy\n' +
      'so a Next.js application can import it. Two copies answering differently is the failure §31\n' +
      'exists to prevent, and the corpus cannot see it. Run with --sync to take the reference\'s.',
  )
  process.exit(1)
}

console.log('\nEvery contract the Hub mirrors matches the reference.')
