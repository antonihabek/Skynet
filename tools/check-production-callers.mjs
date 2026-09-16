/**
 * Every `pub fn` in the workspace is reached by something that is not a test — and no unreached
 * one claims, in its own documentation, that something calls it.
 *
 * # Why this exists
 *
 * This repository has shipped the same defect four times, and it is not a logic error — it is a
 * function that exists, is correct, is tested, and is called by nothing a robot runs:
 *
 * | Symbol | What was true |
 * |---|---|
 * | `Transition::warning` | reached the daemon's observe step and was **dropped**; every rule about the warning tone was enforceable and nothing had ever made a sound |
 * | `can_start` | four test callers and **no production caller**; `skynetk` turned a cycle without ever consulting the start gate |
 * | `EventCode::GrantChanged` | exactly one producer, the reconciler — so the ring recorded every grant a *Hub* made and none an operator made standing at the robot |
 * | `available.proximity_reading` | hard `false` in two files, each carrying the note *"the day a producer exists, this is the line that changes"* |
 *
 * Rust's `dead_code` lint cannot see any of them. A `pub fn` is public, so it is never dead; and a
 * `#[cfg(test)]` module is a real caller as far as the compiler is concerned. Every one of these
 * passed `cargo clippy` with `-D warnings`, passed its own tests, and was found by a person reading
 * code and asking who calls this.
 *
 * # Why it matters more in a dry build
 *
 * `DRY-COVERAGE-PLAN.md` commits to coverage work with no physical robot, so **a fake is the only
 * thing any new code is measured against** — and a fake is satisfied by a function that is merely
 * correct. Hardware is what usually catches a path nobody wired up, by not moving. Removing
 * hardware from the loop removes that, and this check is what stands in for it.
 *
 * # What it does, and what it cannot do
 *
 * It is a parser, not a compiler, and the honest limits are these:
 *
 * * **Test context** is `#[cfg(test)]` and `#[test]` items, brace-matched over text with comments
 *   and string literals removed. A test helper defined inside `#[cfg(test)]` is not reported,
 *   because it is not production surface.
 * * **A use is any mention of the name** in production text outside its own declaration — not a
 *   resolved call. That is deliberately lenient: it under-reports rather than crying wolf, and a
 *   check people switch off finds nothing at all.
 * * **Names are matched without their type**, so two crates with a `pub fn new` satisfy each other.
 *   Ubiquitous names are therefore excluded outright rather than counted unreliably.
 *
 * # The ratchet
 *
 * `production-callers-allow.json` holds what is already unreached, each with a **reason**. The
 * check fails on anything new. Seeding it is not an amnesty: it is the same arrangement as the
 * editions' `--min-passed` floors, where the number is a fact about history and lowering it is a
 * decision somebody makes in a diff rather than something that happens quietly.
 *
 * # The second claim: a doc comment that names a caller it does not have
 *
 * Two of the reasons in that file had to be written **against the function's own documentation**.
 * `check_code` and `tier_profile_to_json` each described a consumer that does not exist — the second
 * said it was *"rendered for a surface that has to explain it"* and there is no such surface — and
 * both were nearly deleted on the strength of it. A comment asserting a caller is the same defect as
 * a figure asserting a measurement, and this half of the check is the same remedy `scale-report.mjs`
 * is for a figure: the claim is held to what the tree can be measured to contain.
 *
 * It runs only over the unreached names, because that is the set where a caller claim is
 * **falsifiable by something already computed here**. On a reached function the claim may be
 * imprecise; it cannot be wrong about whether anybody calls it at all.
 *
 * There is deliberately **no allow-list for this half.** F9 in `DRY-COVERAGE-PLAN.md` records what
 * one costs: the remedy this check offers for a false positive is a name on a list, and that is how
 * a function with a plain production caller came to be denied one in writing. The remedy here is to
 * wire the function up, or to say in the comment that nothing calls it — both of which leave the
 * next reader with something true.
 */

import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const ALLOW_PATH = join(ROOT, 'tools', 'production-callers-allow.json')

/**
 * Names too common to attribute to one crate.
 *
 * Excluded rather than counted, because a `pub fn new` anywhere in the workspace would satisfy
 * every other `pub fn new` and the result would be a check reporting a number nobody can act on.
 * A constructor nobody constructs is caught by the type being unused, which the compiler does see.
 */
const UBIQUITOUS = new Set([
  'new',
  'default',
  'from',
  'from_str',
  'try_from',
  'len',
  'is_empty',
  'as_str',
  'as_bytes',
  'to_string',
  'clone',
  'fmt',
  'parse',
  'get',
  'set',
  'push',
  'next',
  'iter',
  'main',
  'build',
  'run',
  'open',
  'close',
  'read',
  'write',
  'value',
  'name',
  'id',
  'kind',
  'path',
])

/**
 * The things inside this workspace a doc comment might name as a caller.
 *
 * Two halves, and the first is **derived rather than listed** — every crate directory, in both the
 * hyphenated spelling prose uses and the underscored one a `use` line does — so a claim about the
 * next crate somebody adds is covered instead of the ones somebody remembered. The same argument the
 * `unsafe` check in `ci.yml` makes for enumerating the workspace rather than keeping a list beside it.
 *
 * The second half is the role nouns this tree uses for its own parts, and it is a list because there
 * is nowhere to read it from. It grows when somebody writes a claim in a word that is not here yet;
 * that is a gap in the vocabulary and not a licence, and the honest failure direction is this one —
 * a word missing here under-reports, which is the same direction the caller search itself leans.
 *
 * What is deliberately absent is every *outside* consumer: the Hub, a vendor, a driver author, an
 * embedder, a third party's package, the tests. A doc comment naming one of those is making a claim
 * this tool cannot check and has no business refusing — `write_response` is exposed for an embedder
 * running its own loop, `from_flags` is the vendor ABI's own view, and both are correct to say so.
 */
const IN_WORKSPACE = [
  ...readdirSync(join(ROOT, 'crates'))
    .filter((entry) => statSync(join(ROOT, 'crates', entry)).isDirectory())
    .flatMap((crate) => [crate, crate.replace(/-/g, '_')]),
  // The three binaries, which prose names far more often than it names their crates.
  'skynetk',
  'skynetp',
  'skynetd',
  // Role nouns. Each is something in this repository, so "the <noun>" is a checkable claim.
  'runtime',
  'daemon',
  'CLI',
  'console',
  'responder',
  'reconciler',
  'supervisor',
  'launcher',
  'control loop',
  'recorder',
  'safety kernel',
  'kernel host',
  'policy host',
  'black box',
  'blackbox',
]

// Longest first, so `skynetd-io` is not reported as `skynetd` with the rest of the name left off.
// A regex alternation takes the first branch that matches, and the quote below goes in front of a
// person deciding whether the claim is true — a truncated one sends them to the wrong crate.
const SUBJECT = `(?:\`?(?:${IN_WORKSPACE.slice()
  .sort((a, b) => b.length - a.length)
  .map((s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
  .join('|')})\`?)`

/**
 * Constructions that assert a caller exists, as against ones that say what a function is *for*.
 *
 * The distinction the third pattern draws is the **article**, and it is not a nicety. *"rendered for
 * a surface that has to explain it"* is a purpose: it says what the output is shaped for and asserts
 * nothing about whether such a surface has been built. *"For the responder half"* is a claim: it
 * says the responder — this repository's `Responder`, which is right there — is the consumer. The
 * first is how `tier_profile_to_json` should always have read; the second is what `asks_for_us` said
 * while the responder had moved on to a finer question and stopped asking this one.
 *
 * So a definite article in front of something in this workspace is a claim, and an indefinite one is
 * not. That keeps the check off every honest sentence of the form *"for a report"*, *"for a caller
 * running its own loop"*, *"for a driver assembling one"* — three real ones in the tree today.
 */
const CALLER_CLAIMS = [
  ['called by or from', new RegExp(`\\b(?:is |are )?called\\s+(?:by|from)\\s+(?:the\\s+)?${SUBJECT}`, 'i')],
  ['a named caller', new RegExp(`\\bthe\\s+${SUBJECT}\\s+(?:calls|consults|invokes)\\s+(?:this|it)\\b`, 'i')],
  ['the caller named', new RegExp(`\\b(?:its|the)\\s+caller\\s+is\\s+(?:the\\s+)?${SUBJECT}`, 'i')],
  ['for the (definite) consumer', new RegExp(`\\b(?:for|to)\\s+the\\s+${SUBJECT}\\b`, 'i')],
]

/**
 * Sentences that say the caller is absent, which is what makes the claim above honest again.
 *
 * A vocabulary rather than a marker attribute, because both instances of it already in the tree were
 * written by somebody explaining themselves in prose — *"# No surface calls this yet, and it is kept
 * anyway"* on `tier_profile_to_json`, and *"The daemon does not consult this"* on
 * `driver_may_activate`. A `#[doc(hidden)]`-style token would have been a thing to remember; a
 * sentence is a thing a reader gets the benefit of.
 */
const DISCLAIMERS = [
  /\bno\s+[\w-]+\s+(?:calls|consults|reads|renders)\s+(?:this|it)\b/i,
  /\bnothing\s+(?:calls|consults|reads|uses|renders)\s+(?:this|it)\b/i,
  /\bdoes not\s+(?:call|consult|read|use)\s+(?:this|it)\b/i,
  /\bno\s+(?:production\s+|in-workspace\s+)?caller\b/i,
  /\bunreached\b/i,
  /\bnot reached by\b/i,
  /\bbuilt ahead of its\b/i,
  /\bnobody\s+(?:calls|asks|reads)\b/i,
]

/**
 * The `///` lines immediately above a declaration, as one string.
 *
 * Read from the **raw** source, because `strip()` has taken the comments out by the time anything
 * else here looks at a file — and read by line number, which is sound only because `strip()` keeps
 * the newlines of everything it removes. That property is F9's fix, and this is the second thing
 * standing on it: without it every doc comment collected here would belong to some other function.
 */
function docAbove(raw, line) {
  const lines = raw.split('\n')
  const doc = []
  for (let k = line - 2; k >= 0; k--) {
    const text = lines[k].trim()
    if (text.startsWith('///')) doc.unshift(text.slice(3).trim())
    // An attribute or an ordinary comment between the doc and the item does not end it.
    else if (text.startsWith('#[') || text.startsWith('//')) continue
    else break
  }
  return doc.join('\n')
}

/**
 * Remove comments and string literals so brace matching and name search see only code.
 *
 * What is removed is replaced by **the newlines it spanned**, never by nothing. Every line number
 * this file prints is counted off the stripped text, so a block comment or a multi-line string
 * swallowed whole moves every declaration after it: `upload_target` was reported at
 * `skynetd-io/src/upload.rs:545` and is declared at 742, because two hundred lines of module
 * documentation sit between the two. A line number that opens the wrong line is worse than no line
 * number at all, because a reader believes it and goes looking for a defect at the place it names.
 *
 * A separating space goes in as well, so `a/**\/b` and `foo"bar"baz` do not fuse into one token
 * that the declaration pattern could read as a name.
 */
function strip(src) {
  let out = ''
  let i = 0
  const n = src.length
  /** The newlines between two offsets, so removing a span costs its text and not its lines. */
  const spanned = (from, to) => {
    let newlines = ''
    for (let k = from; k < to; k++) if (src[k] === '\n') newlines += '\n'
    return newlines
  }
  while (i < n) {
    const c = src[i]
    const d = src[i + 1]
    // Line comment.
    if (c === '/' && d === '/') {
      while (i < n && src[i] !== '\n') i++
      continue
    }
    // Block comment, nested as Rust allows.
    if (c === '/' && d === '*') {
      const start = i
      let depth = 1
      i += 2
      while (i < n && depth > 0) {
        if (src[i] === '/' && src[i + 1] === '*') {
          depth++
          i += 2
        } else if (src[i] === '*' && src[i + 1] === '/') {
          depth--
          i += 2
        } else i++
      }
      out += ' ' + spanned(start, i)
      continue
    }
    // Raw string: r"..." / r#"..."# / r##"..."## — replaced by a space, never scanned.
    if (c === 'r' && (d === '"' || d === '#')) {
      let j = i + 1
      let hashes = 0
      while (src[j] === '#') {
        hashes++
        j++
      }
      if (src[j] === '"') {
        const start = i
        const terminator = '"' + '#'.repeat(hashes)
        const end = src.indexOf(terminator, j + 1)
        i = end === -1 ? n : end + terminator.length
        out += ' ' + spanned(start, i)
        continue
      }
    }
    // Character literal — `'a'`, `'\n'`, `'\\'`, `'\''`, `'\u{1F600}'`, and the byte form `b'R'`.
    //
    // This is here because of `'"'`. Without it the quote inside that literal opens a string, and
    // the scan runs to the next quote in the file taking real code with it: `upload.rs:230` spells
    // `c == '"'` and five hundred lines below it `upload_target` is called, so this check reported
    // a function with a plain production caller as having none, and it was allow-listed with a
    // reason invented to fit. A false negative here is worse than a false positive, because
    // the answer it gives is the one nobody goes and checks.
    //
    // A lifetime is the thing not to eat: `'a` and `'static` are a quote that no quote closes, so a
    // literal is only recognised when the closing one is where a literal would put it.
    if (c === "'") {
      let j = i + 1
      if (src[j] === '\\') {
        j++
        if (src[j] === 'u' && src[j + 1] === '{') {
          const brace = src.indexOf('}', j)
          j = brace === -1 ? j + 1 : brace + 1
        } else j++
      } else {
        // One scalar value, which is two UTF-16 units above the basic plane.
        const point = src.codePointAt(j)
        j += point !== undefined && point > 0xffff ? 2 : 1
      }
      if (src[j] === "'") {
        out += ' ' + spanned(i, j + 1)
        i = j + 1
        continue
      }
      // Not a literal, so a lifetime or a loop label. Fall through and emit the quote.
    }
    // Ordinary strings. Escapes are honoured.
    if (c === '"') {
      const start = i
      i++
      while (i < n && src[i] !== '"') i += src[i] === '\\' ? 2 : 1
      i++
      out += ' ' + spanned(start, i)
      continue
    }
    out += c
    i++
  }
  return out
}

/**
 * Byte ranges that are test context.
 *
 * An attribute is found, then the item it introduces is brace-matched from its first `{`. An
 * attribute on a `use` or a `const` with no block contributes nothing, which is correct — there is
 * no body to exclude.
 */
function testRanges(code) {
  const ranges = []
  const attr = /#\[\s*(cfg\s*\(\s*test\s*\)|test|cfg_attr\s*\(\s*test)/g
  let m
  while ((m = attr.exec(code)) !== null) {
    let i = code.indexOf('{', m.index)
    const semi = code.indexOf(';', m.index)
    if (i === -1 || (semi !== -1 && semi < i)) continue
    let depth = 0
    const start = m.index
    for (; i < code.length; i++) {
      if (code[i] === '{') depth++
      else if (code[i] === '}') {
        depth--
        if (depth === 0) {
          i++
          break
        }
      }
    }
    ranges.push([start, i])
    attr.lastIndex = i
  }
  return ranges
}

function inRanges(pos, ranges) {
  return ranges.some(([a, b]) => pos >= a && pos < b)
}

/** Blank out the test ranges, preserving offsets so positions stay comparable. */
function productionOnly(code, ranges) {
  let out = code
  for (const [a, b] of ranges) {
    out = out.slice(0, a) + ' '.repeat(b - a) + out.slice(b)
  }
  return out
}

function rustFiles(dir, acc = []) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    const st = statSync(full)
    if (st.isDirectory()) {
      // `tests/` is integration-test surface by Cargo's own convention.
      if (entry === 'target' || entry === 'tests') continue
      rustFiles(full, acc)
    } else if (entry.endsWith('.rs')) acc.push(full)
  }
  return acc
}

const cratesDir = join(ROOT, 'crates')
const files = rustFiles(cratesDir)

/**
 * Files that are test context in their entirety, because a *parent* said so.
 *
 * `#[cfg(test)] mod tests;` puts the whole of `tests.rs` behind the attribute, and nothing inside
 * that file mentions it. Scanning each file alone reported `skynetd-io/src/tests.rs` — nine
 * thousand lines of tests — as production surface, which is the check being wrong in the direction
 * that wastes a reviewer's time rather than the one that hides a defect, and is still wrong.
 */
const testModuleFiles = new Set()
for (const file of files) {
  const code = strip(readFileSync(file, 'utf8'))
  const re = /#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/g
  let m
  while ((m = re.exec(code)) !== null) {
    const dir = dirname(file)
    for (const candidate of [join(dir, `${m[1]}.rs`), join(dir, m[1], 'mod.rs')]) {
      if (existsSync(candidate)) testModuleFiles.add(candidate)
    }
  }
}

/** Declarations, and the production text of every file, in one pass. */
const declared = new Map() // name -> { file, line }
const productionText = []

for (const file of files) {
  const raw = readFileSync(file, 'utf8')
  const code = strip(raw)
  // A file a parent declared behind `#[cfg(test)]` is test context whole, so it neither declares
  // production surface nor counts as a caller.
  const ranges = testModuleFiles.has(file) ? [[0, code.length]] : testRanges(code)
  const prod = productionOnly(code, ranges)
  productionText.push({ file, prod })

  const decl = /\bpub(?:\s*\([^)]*\))?\s+(?:const\s+|async\s+|unsafe\s+|extern\s+"[^"]*"\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)/g
  let m
  while ((m = decl.exec(code)) !== null) {
    if (inRanges(m.index, ranges)) continue
    const name = m[1]
    if (UBIQUITOUS.has(name)) continue
    if (!declared.has(name)) {
      declared.set(name, {
        file: relative(ROOT, file).replace(/\\/g, '/'),
        line: code.slice(0, m.index).split('\n').length,
        declIndex: m.index,
        declFile: file,
      })
    }
  }
}

/** A name is reached if it appears in production text anywhere but its own declaration. */
const unreached = []
for (const [name, where] of declared) {
  const pattern = new RegExp(`\\b${name}\\b`, 'g')
  let reached = false
  for (const { file, prod } of productionText) {
    let m
    pattern.lastIndex = 0
    while ((m = pattern.exec(prod)) !== null) {
      if (file === where.declFile && m.index === where.declIndex + (prod.slice(where.declIndex).indexOf(name))) {
        continue
      }
      // The declaration itself spells the name once; anything else is a use.
      if (file === where.declFile && Math.abs(m.index - where.declIndex) < name.length + 40 && m.index >= where.declIndex) {
        continue
      }
      reached = true
      break
    }
    if (reached) break
  }
  if (!reached) unreached.push({ name, ...where })
}

/**
 * Of the unreached, the ones whose own documentation says something calls them.
 *
 * Computed over every unreached name and not only the allow-listed ones, so a function that is new
 * *and* claims a caller is reported for both faults rather than being fixed once and found again.
 */
const claiming = []
for (const where of unreached) {
  const doc = docAbove(readFileSync(where.declFile, 'utf8'), where.line)
  if (!doc) continue
  if (DISCLAIMERS.some((pattern) => pattern.test(doc))) continue
  for (const [what, pattern] of CALLER_CLAIMS) {
    const hit = pattern.exec(doc)
    if (hit) {
      claiming.push({ ...where, what, quote: hit[0] })
      break
    }
  }
}

const allow = existsSync(ALLOW_PATH)
  ? JSON.parse(readFileSync(ALLOW_PATH, 'utf8'))
  : { note: '', allowed: {} }

const allowed = new Set(Object.keys(allow.allowed ?? {}))
const missingReason = Object.entries(allow.allowed ?? {}).filter(([, r]) => !r || !String(r).trim())
const fresh = unreached.filter((u) => !allowed.has(u.name))
const stale = [...allowed].filter((name) => !unreached.some((u) => u.name === name))

const wantsList = process.argv.includes('--list')
if (wantsList) {
  console.log(`${unreached.length} pub fn with no production caller:\n`)
  for (const u of unreached.sort((a, b) => a.file.localeCompare(b.file))) {
    console.log(`  ${u.file}:${u.line}  ${u.name}`)
  }
  process.exit(0)
}

let failed = false

if (missingReason.length > 0) {
  failed = true
  console.error('::error::Entries in production-callers-allow.json carry no reason.')
  console.error('A name on this list without an argument for being on it is the thing the list')
  console.error('exists to prevent. Say why, or wire the function up.')
  for (const [name] of missingReason) console.error(`  ${name}`)
}

if (fresh.length > 0) {
  failed = true
  console.error('::error::A pub fn has no caller outside a test.')
  console.error('')
  console.error('This is the shape of four defects this repository has already shipped: a function')
  console.error('that is correct, tested, and reached by nothing a robot runs. See the header of')
  console.error('tools/check-production-callers.mjs.')
  console.error('')
  console.error('Either wire it into the path that should call it, or add it to')
  console.error('tools/production-callers-allow.json with the reason it is unreached.')
  console.error('')
  for (const u of fresh) console.error(`  ${u.file}:${u.line}  ${u.name}`)
}

if (stale.length > 0) {
  failed = true
  console.error('::error::production-callers-allow.json lists names that now have production callers.')
  console.error('The ratchet only tightens. Remove them from the file.')
  for (const name of stale) console.error(`  ${name}`)
}

if (claiming.length > 0) {
  failed = true
  console.error('::error::A pub fn with no caller says in its own documentation that it has one.')
  console.error('')
  console.error('A comment asserting a caller is the same defect as a figure asserting a')
  console.error('measurement, and it costs more: the next reader believes it and stops looking.')
  console.error('`tier_profile_to_json` was nearly deleted because its comment named a surface that')
  console.error('does not exist, and the argument for keeping it had to be found in the body.')
  console.error('')
  console.error('Two remedies, and both leave something true behind:')
  console.error('  * wire it into the path the comment names, or')
  console.error('  * say in the comment that nothing calls it yet, and why it is here anyway.')
  console.error('')
  console.error('There is no list to add a name to. See the header of this file for why.')
  console.error('')
  for (const c of claiming) {
    console.error(`  ${c.file}:${c.line}  ${c.name}`)
    console.error(`      ${c.what}: "${c.quote}"`)
  }
}

if (failed) process.exit(1)

console.log(
  `Every pub fn outside the allow-list has a production caller, and no unreached one ` +
    `claims a caller in its documentation (${declared.size} checked, ${allowed.size} allow-listed).`
)
