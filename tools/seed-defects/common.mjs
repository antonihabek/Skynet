/**
 * What the seeding campaign (`seed-defects.mjs`) and the matrices read from it (`matrix.mjs`) share: where things are,
 * how the seeds are loaded and validated, and the digest that says which seed set a report measured.
 * Kept in one place because a digest computed two ways is a report that can never be checked.
 */
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const HERE = dirname(fileURLToPath(import.meta.url))
export const REPO = resolve(HERE, '..', '..')
export const SEEDS_DIR = join(HERE, 'seeds')
/** `SKYNET_SEED_REPORT` points the tools at another report, which is how the gate is shown to fail on a report made worse on purpose. */
export const REPORT = process.env.SKYNET_SEED_REPORT ?? join(REPO, 'reference', 'reports', 'seed-defects.json')

export const suites = JSON.parse(readFileSync(join(HERE, 'suites.json'), 'utf8')).suites
export const classes = JSON.parse(readFileSync(join(HERE, 'classes.json'), 'utf8')).classes

export function die(message) {
  console.error(`seed-defects: ${message}`)
  process.exit(2)
}

export function readReport() {
  return existsSync(REPORT) ? JSON.parse(readFileSync(REPORT, 'utf8')) : { drivers: {} }
}

/** Every driver's seeds, validated: a malformed seed measures nothing, so it stops the run rather than being skipped. */
export function loadSeeds() {
  const all = {}
  for (const file of readdirSync(SEEDS_DIR).filter(name => name.endsWith('.json')).sort()) {
    const doc = JSON.parse(readFileSync(join(SEEDS_DIR, file), 'utf8'))
    if (!suites[doc.driver]) die(`${file} names driver "${doc.driver}", which suites.json does not define.`)
    all[doc.driver] = doc.seeds
    const ids = new Set()
    for (const seed of doc.seeds) {
      for (const key of ['id', 'class', 'file', 'find', 'why']) {
        if (typeof seed[key] !== 'string' || seed[key] === '') die(`${file}: seed ${seed.id ?? '?'} has no "${key}".`)
      }
      // A replacement may be empty: deleting a statement is a defect too.
      if (typeof seed.replace !== 'string') die(`${file}: seed ${seed.id} has no "replace" (an empty string deletes the text).`)
      if (ids.has(seed.id)) die(`${file}: seed id ${seed.id} is used twice.`)
      ids.add(seed.id)
      if (!classes[seed.class]) die(`${file}: seed ${seed.id} names class "${seed.class}", which classes.json does not define.`)
      if (seed.find === seed.replace) die(`${file}: seed ${seed.id} replaces text with itself.`)
      seed.expect = seed.expect ?? 'kill'
      if (!['kill', 'survive'].includes(seed.expect)) die(`${file}: seed ${seed.id} expects "${seed.expect}", not kill or survive.`)
      // An equivalent mutant is a change no test could tell from the original, because nothing observable differs. It is not a
      // control (a control is a real difference nobody can see) and it is not a gap; it is left out of the rate, with the reason
      // written where a reviewer can read it, and it must still survive: an "equivalent" mutant a suite kills was not one.
      if (seed.equivalent !== undefined) {
        if (typeof seed.equivalent !== 'string' || seed.equivalent.trim().length < 40) die(`${file}: seed ${seed.id} calls itself equivalent without saying, in a sentence, why nothing observable differs.`)
        if (seed.expect === 'survive') die(`${file}: seed ${seed.id} is both an equivalent mutant and a control; it is one or the other.`)
        seed.expect = 'survive'
      }
      if (seed.note !== undefined && typeof seed.note !== 'string') die(`${file}: seed ${seed.id}'s note is text.`)
    }
  }
  return all
}

/** A digest of every seed's identity and text, so a report can say which seed set it measured. */
export function digestOfSeeds(all) {
  const hash = createHash('sha256')
  for (const driver of Object.keys(all).sort()) {
    // What the seed *is* — not what has since been decided about it: triaging a survivor as equivalent, or as a real difference no
    // test at this rung can see (`expect: survive` with a note), must not make the run that found it look like a run of different
    // seeds. Only the controls written as such, class `undetectable`, have their expectation in their identity.
    for (const seed of all[driver]) hash.update(JSON.stringify([driver, seed.id, seed.class, seed.file, seed.find, seed.replace, seed.class === 'undetectable' ? 'survive' : 'kill']))
  }
  return hash.digest('hex').slice(0, 16)
}
