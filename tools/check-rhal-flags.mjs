/**
 * §24's state-flag values, in the four places they are written, held to each other.
 *
 * # Why this exists
 *
 * §31.2 keeps the RHAL C ABI single-source *"because two definitions of a struct layout is a
 * segfault"*, and three mechanisms already enforce that for **layout**: `layout_matches_the_header`
 * in `skynet-rhal`, `assert_abi_layout` in the Python binding, and `drivers/mock/header_check.cpp`
 * asserting the same offsets with `offsetof` from the C side.
 *
 * Nothing enforced it for the flag **values**, and they are written down four times:
 *
 * | Where | Why it is a separate copy |
 * |---|---|
 * | `bindings/c/skynet_rhal.h` | the interface, and the authority |
 * | `crates/skynet-rhal/src/lib.rs` | the Rust view, for a Rust driver author |
 * | `bindings/python/skynet_rhal.py` | the Python view, for `drivers/mujoco` and `drivers/lerobot` |
 * | `crates/skynetd-io/src/blackbox.rs` | restated, because §33.1 forbids that crate an edge to `skynet-rhal` and CI asserts the edge is absent |
 *
 * The fourth is the one that makes this necessary rather than tidy. It cannot be removed — an
 * import there would put the supervisor one `use` away from the actuator path — so the duplication
 * is load-bearing and permanent, and §78's rule applies exactly: *the remedy for one fact with
 * several spellings is a mechanism, not care.*
 *
 * # The failure signature, which is why it is a build check
 *
 * A wrong mask does not crash. It reads a **different field**, and both halves go on looking
 * correct: a robot reporting itself homed because the homing mask overlapped `SKR_FLAG_BRAKED`
 * would be a robot whose brakes are on and whose console says it is ready to command. That is a
 * silent, cross-subsystem, safety-relevant wrong answer produced by two files disagreeing about a
 * shift — the exact shape a reviewer does not catch and a parser does.
 *
 * It parses rather than executes for `scale-report.mjs`'s reason: this is a Node script and the
 * authority is a C header. Every source must yield the same name-to-value map, and a name present
 * in one and missing from another is a failure too — a Python driver that cannot spell
 * `HOMING_NOT_REQUIRED` reports `unreported` for ever and nothing anywhere says so.
 */

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Fold a source's own spellings onto the C names, which are the authority.
 *
 * Rust and Python both drop the `SKR_` prefix — `flags::ESTOP`, `FLAG_ESTOP` — because neither
 * language needs C's flat namespace. That is a deliberate difference and not a divergence, so it
 * is normalised here rather than being something each file has to spell C's way.
 */
function canonical(name) {
  return name
    .replace(/^SKR_FLAG_/, '')
    .replace(/^SKR_/, '')
    .replace(/^FLAG_/, '')
    .replace(/^RHAL_/, '')
}

/** Evaluate the shift expressions these files actually contain, and nothing else. */
function value(expression, known) {
  const text = expression.trim().replace(/[u)]/g, '').replace(/\(/g, '')
  // `3 << SHIFT`, `1 << 3`, `0b11 << 4`, or a bare number.
  const shift = /^(0b[01]+|\d+)\s*<<\s*(\w+|\d+)$/.exec(text)
  if (shift) {
    const base = shift[1].startsWith('0b') ? parseInt(shift[1].slice(2), 2) : Number(shift[1])
    const by = /^\d+$/.test(shift[2]) ? Number(shift[2]) : known[canonical(shift[2])]
    if (by === undefined) return undefined
    return base << by
  }
  if (/^0b[01]+$/.test(text)) return parseInt(text.slice(2), 2)
  if (/^\d+$/.test(text)) return Number(text)
  return undefined
}

/**
 * Read one source into a name-to-value map.
 *
 * Two passes, because every one of these files defines its mask in terms of its own shift
 * (`3 << SKR_HOMING_SHIFT`). A single pass would have to depend on declaration order, and a check
 * that breaks when somebody reorders a header is a check people delete.
 */
function parse(file, pattern, also) {
  const source = readFileSync(join(ROOT, file), 'utf8')
  const raw = []
  for (const match of source.matchAll(pattern)) raw.push([canonical(match[1]), match[2]])
  const known = {}
  for (let pass = 0; pass < 2; pass += 1) {
    for (const [name, expression] of raw) {
      const resolved = value(expression, known)
      if (resolved !== undefined) known[name] = resolved
    }
  }
  if (also) Object.assign(known, also(source, known))
  const unresolved = raw.filter(([name]) => known[name] === undefined).map(([name]) => name)
  if (unresolved.length > 0) {
    throw new Error(
      `${file}: this check could not evaluate ${unresolved.join(', ')}. The expression is one `
      + 'this parser does not know, which means the check has stopped covering that constant — '
      + 'teach it the form rather than removing the constant.')
  }
  return known
}

/**
 * `enum Homing`'s discriminants, shifted into the field, as if they were C's constants.
 *
 * `NotRequired = 1` in a two-bit field at shift 4 is `SKR_HOMING_NOT_REQUIRED`. Reading the
 * discriminants rather than trusting them is the point: the enum's `to_flags` does exactly this
 * shift, so a discriminant that drifted from the header would make every Rust driver report the
 * wrong state with no other symptom.
 *
 * `UNREPORTED` is deliberately not produced. It has no variant, because it is `None`.
 */
function enumStates(source, known) {
  const block = /pub enum Homing \{([\s\S]*?)\n\}/.exec(source)
  if (!block) {
    throw new Error(
      'crates/skynet-rhal/src/lib.rs: `enum Homing` is where the Rust edition spells §24\'s '
      + 'homing states, and this check cannot find it. If it moved or became four constants, '
      + 'this parse has to follow it rather than be dropped.')
  }
  const shift = known.HOMING_SHIFT
  const found = {}
  for (const match of block[1].matchAll(/^\s*(\w+)\s*=\s*(\d+),/gm)) {
    // `NotRequired` in the header's spelling is `NOT_REQUIRED`.
    const name = match[1].replace(/([a-z])([A-Z])/g, '$1_$2').toUpperCase()
    found[`HOMING_${name}`] = Number(match[2]) << shift
  }
  return found
}

/**
 * Constants whose absence from a source is correct, with the reason.
 *
 * Only `HOMING_UNREPORTED`, and only in Rust: it is the zero of a two-bit field, and the Rust
 * edition represents it as `Option::None` so that "the driver did not answer" cannot be mistaken
 * for a state a driver reported. A constant for it would re-open exactly that.
 */
const ALLOWED_ABSENCES = {
  'crates/skynet-rhal/src/lib.rs': new Set(['HOMING_UNREPORTED']),
}

const SOURCES = [
  {
    file: 'bindings/c/skynet_rhal.h',
    label: 'the C header (the authority)',
    // `#define SKR_FLAG_ESTOP (1u << 0)  /* ... */`
    pattern: /#define\s+(SKR_(?:FLAG|HOMING)_\w+)\s+([^/\n]+)/g,
  },
  {
    file: 'crates/skynet-rhal/src/lib.rs',
    label: 'the Rust view',
    pattern: /pub const (ESTOP|FAULT|DEGRADED|BRAKED|HOMING_\w+):\s*u16\s*=\s*([^;]+);/g,
    // The homing states are an `enum Homing` here rather than four constants, and that is a
    // deliberate improvement on C rather than a gap: the fourth value — `SKR_HOMING_UNREPORTED` —
    // is `Option::None`, so a Rust driver author cannot accidentally treat "the driver did not
    // say" as a state. The check has to know the two spellings are the same fact, or it would
    // push the crate back towards C's flat constants to satisfy a parser.
    also: enumStates,
  },
  {
    file: 'bindings/python/skynet_rhal.py',
    label: 'the Python binding',
    pattern: /^(FLAG_\w+|HOMING_(?:SHIFT|MASK|UNREPORTED|NOT_REQUIRED|PENDING|COMPLETE))\s*=\s*([^#\n]+)/gm,
  },
  {
    file: 'crates/skynetd-io/src/blackbox.rs',
    label: "the daemon's restatement (§33.1 forbids it the import)",
    pattern: /const (RHAL_\w+):\s*u16\s*=\s*([^;]+);/g,
  },
]

/**
 * Constants a source is allowed not to have.
 *
 * The daemon restates only what it reads, and adding the two flags it has no use for would be
 * three unused constants carrying a lint suppression — worse than the gap. So absence is only a
 * failure for the sources that claim to be complete views of the ABI.
 */
const PARTIAL = new Set(["the daemon's restatement (§33.1 forbids it the import)"])

function main() {
  const parsed = SOURCES.map((source) => ({
    ...source,
    values: parse(source.file, source.pattern, source.also),
  }))
  const [authority, ...rest] = parsed
  const failures = []

  if (Object.keys(authority.values).length === 0) {
    failures.push(
      `${authority.file}: no SKR_FLAG_* or SKR_HOMING_* constants were found at all. This check `
      + 'reads the header as the authority; if the definitions moved, the parse has to move with '
      + 'them rather than pass on an empty set.')
  }

  for (const source of rest) {
    for (const [name, expected] of Object.entries(authority.values)) {
      const actual = source.values[name]
      if (actual === undefined) {
        const allowed = ALLOWED_ABSENCES[source.file]
        if (!PARTIAL.has(source.label) && !(allowed && allowed.has(name))) {
          failures.push(
            `${source.file}: does not define ${name}, which ${authority.file} does. A driver `
            + 'written against this binding cannot report it, and nothing else would say so.')
        }
        continue
      }
      if (actual !== expected) {
        failures.push(
          `${source.file}: ${name} is ${actual} and ${authority.file} says ${expected}. `
          + 'A wrong value here reads a different field and both halves look correct.')
      }
    }
    for (const name of Object.keys(source.values)) {
      if (authority.values[name] === undefined) {
        failures.push(
          `${source.file}: defines ${name}, which ${authority.file} does not. The header is the `
          + 'interface; a constant that exists only in a binding is one no driver author can rely on.')
      }
    }
  }

  // The property the whole homing field rests on: it must not overlap the four flags. Asserted
  // here as well as in `skynet-rhal`'s suite because this is the file that would catch it if
  // somebody added a fifth flag rather than if somebody moved the homing field.
  // Derived from the header rather than listed here, because the flag this would catch is the
  // one somebody adds *next*. A hardcoded list of the four that exist today would go stale in
  // exactly the commit that introduced the collision, and would then pass.
  const flags = Object.entries(authority.values)
    .filter(([name]) => !name.startsWith('HOMING_'))
    .map(([, bit]) => bit)
    .reduce((a, b) => a | b, 0)
  const mask = authority.values.HOMING_MASK
  if (mask !== undefined && (flags & mask) !== 0) {
    failures.push(
      `bindings/c/skynet_rhal.h: the homing field (mask ${mask}) overlaps the state flags `
      + `(${flags}). A braked robot would read as homed, and each half would look correct.`)
  }

  if (failures.length > 0) {
    console.error('§24\'s flag values disagree between their definitions:\n')
    for (const failure of failures) console.error(`  ${failure}`)
    console.error(
      '\nThe header is the interface. Correct the copies, not this check — the duplication is '
      + 'load-bearing (§33.1) and cannot be removed.')
    process.exit(1)
  }

  const names = Object.keys(authority.values).sort()
  console.log(
    `§24's ${names.length} flag constants agree across ${parsed.length} definitions.`)
  for (const name of names) {
    const where = parsed
      .filter((source) => source.values[name] !== undefined)
      .length
    console.log(`  ${String(authority.values[name]).padStart(3)}  ${name}  (${where} of ${parsed.length})`)
  }
}

main()
