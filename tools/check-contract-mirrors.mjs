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
 * # With one repository, which is what CI has
 *
 * The comparison above needs both trees, and for a long time this script's answer to having one was
 * to print "BotMarket not found" and exit zero — so the CI job named after this check has never
 * been able to fail, and the one divergence it has actually caught was caught by a human running it
 * locally.
 *
 * So the digests are pinned in **both** repositories, written by the same `--sync`. With one
 * checkout the script verifies the reference against its own pins, which catches the failure that
 * matters most here: a mirrored contract edited without the Hub being resynced. A fresh clone
 * matches its pins and passes, so a vendor who has cloned only this repository is still never told
 * their tree is broken — the check fires when somebody edits a contract, and says what that costs.
 */

import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Modules the Hub keeps its own copy of.
 *
 * `ota` (§12) is here because `GET /os/releases` serves the manifests `admitBundle` judges, and the
 * Hub is the party that can say *"this bundle will be gated on a bench pass"* **before** somebody
 * publishes it rather than after a fleet has refused it. The vocabulary is the sharper half:
 * `OS_COMPONENTS` is an allow-list that fails **closed** — an unrecognised component is treated as
 * though it were the safety kernel, because a publisher shipping a kernel change past the gate
 * would write `core` or `rt` rather than `safety_kernel`. A Hub validating `changes` against its
 * own list would be a second, laxer copy of the one vocabulary in this system whose whole design is
 * that it is strict.
 *
 * What the Hub does **not** get from mirroring it is the decision. §11 forbids a control decision
 * requiring a network round trip, so whether a bundle may be applied is answered on the robot,
 * every time, from resident facts. The Hub computes the same verdict for a person to read.
 *
 * `calibration` (§45) joined when the table it was named as waiting on landed, and its reason is
 * §45.2's table read as a deliverable: `guardsUnavailable` converts *"we should calibrate the
 * cameras sometime"* into *"an expired extrinsic disables the proximity guard"*, which is a sentence
 * §32.1's device page asks for by name — *"active and unavailable guards with the missing input
 * named"*. Both sides compute it from declared facts and neither is authoritative, which is the
 * test this list applies.
 *
 * `watchdog` (§46), `entitlement` (§51), `skill` (§54) and `power` (§36) are deliberately **not** here yet. Each is a contract the Hub will need — a fleet page that cannot
 * say which robots are running on a lapsed entitlement is a fleet page missing the one commercial
 * fact §51 defines — and each waits on a surface that does not exist. Mirroring them now would put
 * four modules nothing imports into an application, and the honest place to record that they are
 * coming is here rather than in somebody's memory.
 *
 * `power` is the newest of the four and has the sharpest trigger: the Hub's robot registry holds
 * the descriptor, so the moment a robot page wants to say *"this body declares a pack and no
 * safe-stop energy, so §36.2's reserve has no boundary"* — which is a readiness question, asked of
 * declared facts, on the Hub, before a robot is anywhere near it — both sides need to be computing
 * the same `assess`. Today nothing on the Hub reads a `power` block at all, so a mirror would be a
 * copy with no reader, which is the state this list exists to avoid.
 *
 * `birth` (§50) was the fifth name in that paragraph until §57's `POST /robots/{slug}/attestations`
 * was written, and it is the clearest case in this list of why the test is *both sides must agree*
 * rather than *the Hub touches it*. A vendor signs a certificate at manufacture; the Hub reads that
 * same document at pairing, months later and on another continent; an operator reads it again from
 * a check-in. Nothing about the certificate changes in between — so the three readings are the same
 * function over the same bytes, and a Hub with its own copy of `check` would be a fourth opinion
 * about what a document proves. The asymmetry §50.2 insists on survives the mirroring: the Hub
 * decides the **signature state**, because only it holds the vendor's registered key, and then runs
 * the shared function over it. Deciding and checking are two jobs, and only the second is mirrored.
 *
 * `provenance` (§56) is the newest, and it is the one entry on this list where the Hub is the
 * **consumer** rather than a second opinion: §56.2 says the Hub assigns the class, and the robot's
 * edition computes it only so a robot can predict its own offline. That inverts the usual test and
 * makes the case for mirroring stronger rather than weaker — two implementations of an anti-gaming
 * rule that disagree is an anti-gaming rule with a hole, and a hole in this one always points the
 * same way, towards admitting something.
 *
 * What it waited on was a schema decision rather than an endpoint, and the decision turned out to
 * be that there was nothing to reconcile. `skynet_evaluations` carries `verified: boolean` and
 * `source: platform | self_reported | imported`; §56.2 asks for `field | simulated | unattested`.
 * Those are **two axes, not two spellings of one**: the first says who submitted a number, the
 * second says what it is evidence of. So `provenance` is a new nullable column beside them rather
 * than a replacement for either, and NULL means §56 has not classified this run — which is true of
 * every row that arrived without a run bundle, and is not the same fact as `unattested`.
 *
 * `entitlement` has a second reason to wait, and it is the more interesting one. §57 makes the
 * token *"a field in the desired-state document, not an endpoint"*, so the Hub's job is to **issue**
 * tokens and the robot's is to decide about them. Those are not the same function, and mirroring
 * the robot's decision into the Hub before anything issues one would put the wrong half there.
 *
 * `cell` (§55.2) is different from every other name in this paragraph: it is not waiting, it is
 * **never coming**. §55.2 refuses to make a cell a repo kind at all — *"making it a repo kind would
 * invite people to publish their factory layout to a marketplace, and would put safety-relevant
 * workspace volumes behind a fork-and-star workflow"* — so a module the Hub imported would be a
 * module arguing the opposite of the section it implements. It stays in the reference because the
 * robot has two editions of it and they have to agree; it stays out of the Hub because the Hub is
 * the marketplace.
 *
 * `skill` is the one whose turn is closest, and it waits on a page rather than on a table. It
 * answers questions a repository page will want — *which of this skill's preconditions can the OS
 * actually enforce, and which are still sentences?* — and that page needs UI and thirty-five
 * locale catalogues, which is a piece of work rather than a line in this list. Mirroring it before
 * the page exists would put a module nothing imports into an application, which is the same
 * mistake in the other direction.
 *
 * `predicate` and `semver` are the newest entries and the first that arrived from the **Hub**
 * rather than from this repository. §54.2 turns a skill's `preconditions` from prose into
 * predicates the robot evaluates before the skill may start, and `semver_satisfies` is one of the
 * operators — so a robot deciding whether its runtime satisfies `^1.2.0` differently from the Hub
 * that sold it the skill is §31.2's failure exactly: *a format with two implementations of its own
 * definition is not a format.* The engine had lived at `lib/skynet/compatibility/predicate.ts`
 * since long before Part VI was written; it is now the reference's, and the Hub's two old paths are
 * re-exports of `lib/skynet/os/`.
 *
 * That move is the reason to be careful with `semver`: it is one of the most-imported modules in
 * that repository, and a mirrored contract cannot be edited there in passing any more.
 *
 * `opstate` (§27) is not here, and the table it was waiting on has now landed —
 * `skynet_device_state` exists and a check-in writes §27's state into it. It stays out anyway, on
 * the second of the two reasons it always had, which is the one that survives the table.
 *
 * Every module in the list above answers a question **from declared facts**, which is why both sides
 * can compute it and neither is authoritative. §27's machine answers from *observations of a body*,
 * and the Hub has none: it would be running the evaluator over conditions a robot sent it, which is
 * displaying a robot's answer rather than computing a second opinion. That is a renderer, not a
 * mirrored contract, and mirroring it would suggest a disagreement between the two was possible.
 *
 * `telemetry` (§74) is here because the Hub is the **only** consumer of `coverage`. A robot
 * produces windows and never reads a set of them back; a Hub divides §15's field conformance by
 * what the windows actually covered rather than by the time that elapsed, and that difference — one
 * hour of windows out of a day is not a demonstration about the other twenty-three — is the
 * difference between suspending a robot's certification and not. It is mirrored rather than written
 * here because `assess_window` runs on both sides and a Hub that counted a window the robot would
 * have refused would be holding evidence the robot never claimed to have produced.
 *
 * `identity`, `checkin` and `intent` are the three that joined before it, and each earns it from a
 * different direction — which is worth writing down, because "the Hub touches this now" is not the test.
 *
 *   * `identity` because **both sides must agree on what a pairing code is**. The robot folds `O`
 *     to `0` before it sends one (§18's alphabet has no `O`), and a Hub that minted codes from a
 *     different alphabet, or compared without folding, would refuse codes an operator typed exactly
 *     as printed. `DEVICE_SCOPES` is the same property from the other end: the robot refuses a
 *     credential carrying a scope outside the list, so the Hub minting one outside it is a robot
 *     that pairs and then reports itself broken.
 *   * `checkin` because the Hub is the **only** consumer of `claimFor`'s four states, and §17's
 *     rule — *the platform never claims a robot is running something it has not heard from* — is
 *     enforced by whichever side decides what `applied: false` means. Two implementations of that
 *     is two answers.
 *   * `intent` because `assessOffer`'s own header asks for it: *"the same function runs on the robot
 *     and on a Hub checking what it just served."* A Hub that can tell an operator "this robot will
 *     refuse this document, and here is why" before serving it is the only place the
 *     `generation_reused` refusal can be *prevented* rather than reported.
 *
 * Add a name to this list and run `--sync`; nothing else is needed.
 */
const MIRRORS = [
  'birth', 'calibration', 'canonical', 'checkin', 'conformance', 'digest', 'embodiment',
  'identity', 'intent', 'kinematics', 'ota', 'package', 'predicate', 'provenance', 'readiness',
  'safety', 'semver', 'telemetry',
]

const REFERENCE = 'reference/typescript'
const HUB = 'lib/skynet/os'

/**
 * The wire fixtures, which are mirrored for a different reason from the contracts above.
 *
 * A contract is mirrored so two implementations of one rule cannot drift. A **fixture** is mirrored
 * so a producer and a consumer that live in different repositories can be tested against each
 * other: each file is the exact canonical body `skynetd` puts on the wire for one §17 route,
 * produced by the real producer and frozen by `skynetd-io`'s suite.
 *
 * The twenty-seventh session is the argument for it. Both repositories had built the episode path —
 * a producer, a queue, a route, a table — and had never put one document through the other. They
 * did not agree about a single field name, `POST /episodes` ignores what it does not know, and both
 * suites passed the whole time, because one asserted what the robot sends and the other asserted
 * what the Hub accepts.
 *
 * See `reference/wire/README.md`, including what is deliberately *not* frozen here.
 */
const WIRE = ['checkin', 'episode', 'incident', 'telemetry']

const WIRE_REFERENCE = 'reference/wire'
const WIRE_HUB = 'lib/skynet/os/wire'

const args = process.argv.slice(2)
const sync = args.includes('--sync')
const given = args.find(arg => !arg.startsWith('--'))

// The Hub was renamed from BotMarket to Robot Overflow, and both spellings are here because a
// checkout made before the rename is still a valid checkout. A tool that could only find the old
// name is a tool that prints "not found" forever and quietly stops comparing anything.
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

/** The digest of one reference contract, as the Hub would hold it. */
function digestOf(name) {
  const path = join(HERE, REFERENCE, `${name}.ts`)
  if (!existsSync(path)) return null
  return createHash('sha256').update(toHub(readFileSync(path, 'utf8'))).digest('hex')
}

/**
 * The digest of one wire fixture.
 *
 * No rewriting: a fixture crosses as the bytes it is, because the bytes *are* the subject. The only
 * normalisation is the line ending, for the reason `lf` gives.
 */
function wireDigestOf(name) {
  const path = join(HERE, WIRE_REFERENCE, `${name}.json`)
  if (!existsSync(path)) return null
  return createHash('sha256').update(lf(readFileSync(path, 'utf8'))).digest('hex')
}

/** Both sets, keyed the way the pin file holds them. */
function allDigests() {
  const digests = {}
  for (const name of MIRRORS) {
    const digest = digestOf(name)
    if (digest !== null) digests[name] = digest
  }
  for (const name of WIRE) {
    const digest = wireDigestOf(name)
    // Namespaced, so a contract and a fixture with the same stem cannot collide in one flat map.
    if (digest !== null) digests[`wire/${name}`] = digest
  }
  return digests
}

/**
 * The pinned digests, kept in **both** repositories.
 *
 * The comparison further down only runs where both are checked out, and CI has one. So the job that
 * exists to catch a divergence has never been able to: it printed "BotMarket not found" and exited
 * zero, and the one defect this tool has actually caught was caught by a human running it locally.
 *
 * This file closes that. It holds the same digests as the Hub's copy and is written by the same
 * `--sync`, so the two cannot drift from each other. What it buys is the property the Hub already
 * had and this repository did not: **each side fails in its own suite, with one checkout.**
 *
 *   * The Hub's copy catches its own files being edited in place — which is what a well-meaning fix
 *     to a type error in a Next.js application looks like.
 *   * This copy catches a **reference contract changing without the Hub being resynced**, which is
 *     the direction that matters more: the reference is the oracle the corpus holds the Rust
 *     edition to, so a change here is a change every edition owes.
 *
 * It is not "your tree is broken" for somebody who cloned only this repository — a fresh checkout
 * matches its own pins. It fires exactly when a mirrored contract has been edited, and says what
 * that costs.
 */
const PINS = join(HERE, REFERENCE, 'mirror-digests.json')

function checkPins() {
  if (!existsSync(PINS)) {
    console.error(`\n${REFERENCE}/mirror-digests.json is missing. Run with --sync to write it.`)
    return 1
  }
  const pinned = JSON.parse(readFileSync(PINS, 'utf8')).digests ?? {}
  let wrong = 0
  for (const [name, digest] of Object.entries(allDigests())) {
    if (pinned[name] === digest) continue
    const what = name.startsWith('wire/') ? 'fixture' : 'contract'
    console.error(
      pinned[name]
        ? `CHANGED  ${name} — this ${what} no longer matches its pinned digest.`
        : `UNPINNED ${name} — this ${what} has no pinned digest.`,
    )
    wrong += 1
  }
  if (wrong > 0) {
    console.error(
      `\n${wrong} mirrored file(s) changed without the Hub being resynced.\n`
        + 'The reference is the oracle the corpus holds the Rust edition to, and the Hub keeps its\n'
        + 'own copy so a Next.js application can import it. A change here is a change the Hub owes,\n'
        + 'and the two have to land together:\n\n'
        + '    node tools/check-contract-mirrors.mjs --sync ../BotMarket\n\n'
        + 'Editing the pins by hand switches this check off.',
    )
  }
  return wrong
}

/** Write the pins into this repository, and into the Hub when it is there, from one computation. */
function writePins(hubPath) {
  const digests = allDigests()
  const body = `${JSON.stringify({
    note: 'sha256 of each reference contract, with its imports rewritten for the Hub, and of each '
      + 'wire fixture as-is under a `wire/` prefix. Written by '
      + 'tools/check-contract-mirrors.mjs --sync in the Skynet OS repository; checked there against '
      + 'reference/typescript/, and in BotMarket by lib/__tests__/skynet-os-mirrors.test.ts. Never '
      + 'edit by hand — a digest somebody updated to match a local edit is a check that has been '
      + 'switched off.',
    digests,
  }, null, 2)}\n`

  writeFileSync(PINS, body, 'utf8')
  console.log(`wrote    ${REFERENCE}/mirror-digests.json`)
  if (hubPath) {
    writeFileSync(join(hubPath, HUB, 'mirror-digests.json'), body, 'utf8')
    console.log(`wrote    ${HUB}/mirror-digests.json`)
  }
}

if (!existsSync(hub)) {
  // The comparison needs both trees. The pins do not, and one repository is the case they were
  // added for, because it is the case CI runs in.
  console.log(`BotMarket not found at ${hub} — the Hub's copies cannot be compared.`)
  if (sync) {
    writePins(null)
    process.exit(0)
  }
  const wrong = checkPins()
  if (wrong > 0) process.exit(1)
  console.log('Every mirrored contract matches its pinned digest.')
  process.exit(0)
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

for (const name of WIRE) {
  const ours = join(HERE, WIRE_REFERENCE, `${name}.json`)
  const theirs = join(hub, WIRE_HUB, `${name}.json`)

  if (!existsSync(ours)) {
    console.log(`SKIP     wire/${name} — no reference copy`)
    continue
  }
  const expected = lf(readFileSync(ours, 'utf8'))

  if (!existsSync(theirs)) {
    if (sync) {
      mkdirSync(dirname(theirs), { recursive: true })
      writeFileSync(theirs, expected, 'utf8')
      console.log(`created  ${WIRE_HUB}/${name}.json`)
      synced += 1
    } else {
      console.log(`MISSING  ${WIRE_HUB}/${name}.json — the Hub has no copy of this fixture`)
      missing += 1
    }
    continue
  }

  if (lf(readFileSync(theirs, 'utf8')) === expected) {
    console.log(`match    wire/${name}`)
    continue
  }

  if (sync) {
    writeFileSync(theirs, expected, 'utf8')
    console.log(`synced   ${WIRE_HUB}/${name}.json  <- ${WIRE_REFERENCE}/${name}.json`)
    synced += 1
    continue
  }

  // No line count for a fixture: it is one line of canonical JSON, and the useful output is that
  // the document the robot sends is not the document the Hub is testing against.
  console.log(`DIVERGED wire/${name} — the Hub is holding a different document`)
  diverged += 1
}

/**
 * The digests, written into both repositories.
 *
 * This half catches a different failure from the comparison above, and all three are needed.
 * Running this script with both trees catches *the reference having moved ahead*. The Hub's copy of
 * the pins catches the Hub's own files being edited in place, checked by its suite on a machine
 * that has never heard of this repository. This repository's copy catches a reference contract
 * changing without a resync, on a machine that has never heard of the Hub — which is the case CI
 * actually runs in.
 */
if (sync) {
  writePins(hub)
}

// Checked even when the Hub is present, so `--sync` is not the only thing that can write them and
// a stale pin cannot survive a run that had both trees in front of it.
if (!sync && checkPins() > 0) process.exit(1)

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
