/**
 * Every `pub fn` in the workspace is reached by something that is not a test.
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
    // Byte or char literal, and ordinary strings. Escapes are honoured.
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

if (failed) process.exit(1)

console.log(
  `Every pub fn outside the allow-list has a production caller ` +
    `(${declared.size} checked, ${allowed.size} allow-listed).`
)
