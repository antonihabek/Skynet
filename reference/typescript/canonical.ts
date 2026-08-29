/**
 * Canonical JSON, the TypeScript half.
 *
 * This is a deliberate reimplementation of `crates/skynet-json`, and the duplication is the point.
 * §31.4 requires every edition to reproduce the corpus **byte-identically**, and a corpus whose
 * expected values were produced by the same code that checks them would prove nothing. So the rule
 * is written down once in the specification and implemented once per edition, and the corpus is
 * where the two implementations meet.
 *
 * The rule, restated so this file can be read on its own:
 *
 * 1. No insignificant whitespace.
 * 2. Object keys sorted by the byte sequence of their UTF-8 encoding.
 * 3. Numbers are finite. An integral value below 2^53 is written as an integer, `-0` as `0`;
 *    anything else as the shortest decimal that round-trips, **always positional, never
 *    exponential**. `JSON.stringify` disagrees on that last point — it emits `1e-7` where this
 *    emits `0.0000001` — which is exactly why this function exists instead of a `JSON.stringify`
 *    with a replacer.
 * 4. Strings escape `"`, `\` and the C0 controls and nothing else.
 * 5. `undefined` fields are omitted. `null` is written as `null`. The two are different facts:
 *    the whole system distinguishes "nobody declared a safety envelope" from "an envelope was
 *    declared and is empty".
 */

const INT_SAFE = 9007199254740992 // 2^53
const encoder = new TextEncoder()

/** Compare two strings by their UTF-8 byte sequences, ascending. */
function compareUtf8(a: string, b: string): number {
  const left = encoder.encode(a)
  const right = encoder.encode(b)
  const shared = Math.min(left.length, right.length)
  for (let i = 0; i < shared; i += 1) {
    if (left[i] !== right[i]) return left[i] < right[i] ? -1 : 1
  }
  return left.length - right.length
}

/**
 * Turn `-4.4e-1` into `-0.44`.
 *
 * `toExponential` always renders as `<digits>e<sign><exp>`, which is the only shape this handles.
 */
function exponentialToPositional(rendered: string): string {
  const marker = rendered.indexOf('e')
  if (marker < 0) return rendered

  let mantissa = rendered.slice(0, marker)
  const exponent = Number(rendered.slice(marker + 1))

  let negative = false
  if (mantissa.startsWith('-')) {
    negative = true
    mantissa = mantissa.slice(1)
  }
  const digits = mantissa.replace('.', '')
  const digitCount = digits.length

  // The first digit carries weight 10^exponent, so the point falls after `exponent + 1` digits.
  const point = exponent + 1

  let out = negative ? '-' : ''
  if (point <= 0) {
    out += '0.' + '0'.repeat(-point) + digits
  } else if (point >= digitCount) {
    out += digits + '0'.repeat(point - digitCount)
  } else {
    out += digits.slice(0, point) + '.' + digits.slice(point)
  }
  return out
}

/** Format a number in canonical form. Throws on a non-finite value rather than emitting `null`. */
export function formatNumber(value: number): string {
  if (!Number.isFinite(value)) {
    throw new Error(`cannot serialise a non-finite number: ${value}`)
  }
  if (value === 0) return '0' // covers -0
  if (Number.isInteger(value) && Math.abs(value) < INT_SAFE) return String(value)

  for (let precision = 1; precision <= 17; precision += 1) {
    const rendered = value.toExponential(precision - 1)
    if (Number(rendered) === value) return exponentialToPositional(rendered)
  }
  return exponentialToPositional(value.toExponential(16))
}

function writeString(value: string, out: string[]): void {
  out.push('"')
  for (const ch of value) {
    switch (ch) {
      case '"': out.push('\\"'); break
      case '\\': out.push('\\\\'); break
      case '\b': out.push('\\b'); break
      case '\f': out.push('\\f'); break
      case '\n': out.push('\\n'); break
      case '\r': out.push('\\r'); break
      case '\t': out.push('\\t'); break
      default: {
        const code = ch.codePointAt(0) ?? 0
        if (code < 0x20) {
          out.push('\\u' + code.toString(16).padStart(4, '0'))
        } else {
          out.push(ch)
        }
      }
    }
  }
  out.push('"')
}

function writeValue(value: unknown, out: string[]): void {
  if (value === null) {
    out.push('null')
    return
  }
  switch (typeof value) {
    case 'boolean':
      out.push(value ? 'true' : 'false')
      return
    case 'number':
      out.push(formatNumber(value))
      return
    case 'string':
      writeString(value, out)
      return
    case 'undefined':
      // Only reachable as an array element; an object's undefined fields are dropped below.
      out.push('null')
      return
  }

  if (Array.isArray(value)) {
    out.push('[')
    value.forEach((item, index) => {
      if (index > 0) out.push(',')
      writeValue(item, out)
    })
    out.push(']')
    return
  }

  const record = value as Record<string, unknown>
  const keys = Object.keys(record)
    .filter(key => record[key] !== undefined)
    .sort(compareUtf8)
  out.push('{')
  keys.forEach((key, index) => {
    if (index > 0) out.push(',')
    writeString(key, out)
    out.push(':')
    writeValue(record[key], out)
  })
  out.push('}')
}

/** Serialise a value in canonical form. */
export function canonical(value: unknown): string {
  const out: string[] = []
  writeValue(value, out)
  return out.join('')
}
