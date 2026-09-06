/**
 * §10 — identity, pairing and attestation.
 *
 * Three decisions, and they are three because they are answered by three different parties:
 *
 * 1. **Is this eight characters somebody typed a pairing code?** Answered on the robot, before any
 *    network exists, by `normalizePairingCode`.
 * 2. **Is the credential that came back the class §10 asked for?** Answered on the robot, against
 *    the credential itself, by `checkIdentity`.
 * 3. **What is this robot running?** Answered by `attest`, which produces the sentence §10 says an
 *    incident investigation needs and nobody can currently produce.
 *
 * # The two artifacts, and why the first check is that they were not confused
 *
 * Pairing is an exchange of one short-lived secret for one long-lived one: a **pairing code** goes
 * to the Hub and a **device credential** comes back. Both are bearer secrets, both are strings, and
 * an operator holding both will eventually paste the wrong one into `skynet pair`. A credential
 * presented as a code is sent to a Hub in a field nobody expected a credential in, and lands in
 * whatever log that Hub keeps of failed pairings — so it is refused here, by name, before anything
 * is sent.
 *
 * # No refusal ever echoes the code
 *
 * A pairing code is a bearer secret for the length of a pairing. A refusal that quotes it puts it
 * in a terminal scrollback, a shell history and the audit log of a robot the operator is standing
 * in front of. So the faults below carry a **position** and never a character, and never the input.
 * That costs the operator a little — "character five is not one a code uses" is worse advice than
 * naming the character — and it is the right trade for a secret whose whole lifetime is the minute
 * somebody is typing it.
 *
 * # There is no check character, and that is a decision rather than an omission
 *
 * §18 writes a code out in full: `skynet pair ABCD-1234`. Eight characters in two groups of four,
 * and no room in them for a checksum. A check character could be added and would turn a mistyped
 * code into a local refusal instead of a round trip — and it would make the specification's own
 * example invalid, which is this file overruling the document for the convenience of a better error
 * message. So the local check is the **shape**, and whether a well-shaped code exists is the Hub's
 * question, because the Hub is the only party that can answer it.
 *
 * # Pure
 *
 * No clock, no network, no secure element. Whether a key lives in one arrives as an observation, the
 * way a signature state arrives decided in §37.1 — the robot's own answer to that question and the
 * Hub's answer from a check-in are then the same function over the same facts.
 */

/**
 * The alphabet a pairing code is drawn from — Crockford's base32.
 *
 * Excludes `I`, `L`, `O` and `U`. The first three because a code is read off a screen and typed on a
 * keyboard, and 1/I/l and 0/O are the two confusions that account for most of it; `U` because
 * Crockford drops it so that a random code cannot spell something an operator has to read aloud to
 * a colleague.
 *
 * The excluded letters are not rejected on input — they are **folded**, because an operator who
 * types the letter they saw should get the code they saw.
 */
export const PAIRING_ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'

/**
 * How many characters a pairing code has, hyphens and spacing removed.
 *
 * Eight, from §18's `skynet pair ABCD-1234`. Forty bits, which is a large number of guesses against
 * a code that lives for minutes behind a Hub that rate-limits, and a small number of characters for
 * somebody reading them off a screen — which is the trade the length is actually making.
 */
export const PAIRING_CODE_LENGTH = 8

/** How a pairing code is written for a person to read: two groups of four. */
export const PAIRING_CODE_GROUP = 4

/**
 * The prefix on a Hub device credential (§10).
 *
 * Here so that a credential pasted into `skynet pair` is recognised as one and refused by name.
 */
export const DEVICE_TOKEN_PREFIX = 'skd_'

export const CODE_FAULTS = [
  'empty',
  'looks_like_a_credential',
  'too_short',
  'too_long',
  'not_in_alphabet',
] as const

export type CodeFaultCode = (typeof CODE_FAULTS)[number]

/**
 * Why this is not a pairing code.
 *
 * A nested object rather than three sibling fields, and the reason is the shape the rest of the
 * corpus already uses: a `fault` that is either `null` or `{ code, detail, … }` is what frame
 * validation and the cell descriptor emit, and it is the shape the §64 refusal check reads. Flat
 * fields would have made every refusal in this file invisible to that check while looking exactly
 * as correct — which is the failure mode a cross-cutting check exists to catch, and it caught this
 * one.
 */
export interface CodeFault {
  code: CodeFaultCode
  /** The sentence an operator gets. Never contains the input. */
  detail: string
  /** 1-based, into the normalized code. `null` unless the code is `not_in_alphabet`. */
  position: number | null
}

/** What a robot made of the characters somebody typed. */
export interface CodeResult {
  /** The normalized code, or `null` when there is a fault. Upper case, no hyphens. */
  code: string | null
  /** What is wrong, or `null` when nothing is. */
  fault: CodeFault | null
}

/** Crockford's folding: the letters excluded from the alphabet map to the digits they resemble. */
function fold(character: string): string {
  switch (character) {
    case 'O': return '0'
    case 'I': return '1'
    case 'L': return '1'
    default: return character
  }
}

/**
 * Turn what somebody typed into a pairing code, or say why it is not one.
 *
 * The order of the checks is the order of usefulness, and it is not the order they would fall out
 * of a parser. A credential pasted here is caught **before** anything is stripped, because `skd_`
 * contains an underscore and would otherwise be reported as an unusable character at position four
 * — which is true, useless, and would have the operator hunting for a typo in a string that has
 * none. Length is checked before the alphabet for the same reason: somebody who typed seven
 * characters needs to be told they typed seven.
 */
export function normalizePairingCode(input: string): CodeResult {
  if (input.trim().toLowerCase().startsWith(DEVICE_TOKEN_PREFIX)) {
    return {
      code: null,
      fault: {
        code: 'looks_like_a_credential',
        detail: `This is a device credential — it begins \`${DEVICE_TOKEN_PREFIX}\` — and pairing `
          + 'takes the short code the Hub showed you, not the credential it hands back afterwards. '
          + 'Nothing has been sent. Treat the credential as exposed if it has been on a screen '
          + 'somebody else could see.',
        position: null,
      },
    }
  }

  // Two loops rather than one, and both of them are code points.
  //
  // `toUpperCase()` on one code point can return more than one — `ß` is `SS`, `ﬁ` is `FI` — so
  // folding its *result* asks `fold` a question about a string when every case it answers is about
  // a character. This file did that until 2026-09-05, and the cost was not theoretical: `ﬁ`
  // uppercased to `FI` and stayed `FI` here, while the Rust edition folded each character and got
  // `F1` — two characters that are both **in** the alphabet. So a code containing a ligature was
  // refused by the Hub and accepted by the robot, which is the §17 disagreement in its worst
  // direction: the half that decides is the half that says yes.
  let normalized = ''
  for (const character of input) {
    if (character === '-' || character === ' ' || character === '\t'
      || character === '\n' || character === '\r') continue
    for (const upper of character.toUpperCase()) normalized += fold(upper)
  }

  // Code points, not UTF-16 code units, for everything below: the length in the message and the
  // position in the fault. `'𝟶'.length` is 2 and a person who typed one character is not told they
  // typed two — §64 makes a refusal that miscounts what somebody typed a defect, not a rounding
  // error. Indexing matters for the same reason and one worse: `normalized[i]` on an astral
  // character yields half a surrogate pair, which is in no alphabet and is not what anybody typed.
  const characters = Array.from(normalized)

  if (normalized === '') {
    return {
      code: null,
      fault: {
        code: 'empty',
        detail: 'No code was given. `skynet pair` takes the eight-character code the Hub showed '
          + `you, written as ${'X'.repeat(PAIRING_CODE_GROUP)}-${'X'.repeat(PAIRING_CODE_GROUP)}.`,
        position: null,
      },
    }
  }

  if (characters.length !== PAIRING_CODE_LENGTH) {
    return {
      code: null,
      fault: {
        code: characters.length < PAIRING_CODE_LENGTH ? 'too_short' : 'too_long',
        detail: `A pairing code is ${PAIRING_CODE_LENGTH} characters and this is `
          + `${characters.length}. Hyphens and spaces do not count towards it.`,
        position: null,
      },
    }
  }

  for (let index = 0; index < characters.length; index += 1) {
    if (!PAIRING_ALPHABET.includes(characters[index])) {
      return {
        code: null,
        fault: {
          code: 'not_in_alphabet',
          detail: `Character ${index + 1} of the ${PAIRING_CODE_LENGTH} is not one a pairing code `
            + 'uses. Codes are digits and upper-case letters, without I, L, O or U — if you read '
            + 'one of those off the screen it was a 1 or a 0, and typing it that way works too.',
          position: index + 1,
        },
      }
    }
  }

  return { code: normalized, fault: null }
}

/** Render a normalized code the way it is printed for a person. */
export function formatPairingCode(code: string): string {
  const groups: string[] = []
  for (let index = 0; index < code.length; index += PAIRING_CODE_GROUP) {
    groups.push(code.slice(index, index + PAIRING_CODE_GROUP))
  }
  return groups.join('-')
}

// ── §19's device credential ──────────────────────────────────────────────────

/**
 * What a device credential may carry, as this edition reads §17.
 *
 * **The direction is §19's and the vocabulary is not.** §19 says a device credential "cannot read
 * repositories, claim jobs, or write to another namespace", and gives no list of scope names — the
 * Hub has not declared one. So this list is §17's and §57's device-facing routes, one scope each,
 * and it is a **default this edition freezes because a robot has to check something**, in the same
 * sense `DEFAULT_BOOT_BUDGET_MS` is a default rather than a derived constant.
 *
 * What is *not* a default is the direction. This is an allow-list and anything outside it is
 * refused, for exactly the reason §12 gates an unrecognised OS component: a deny-list of §19's
 * three phrases would be got past by a credential that spelled repository read any other way, and
 * the one thing §19 says about this credential is that it is **assumed lost**. A capability nobody
 * has established is narrow, on a credential everybody assumes is stolen, is the combination the
 * whole clause exists to prevent.
 */
export const DEVICE_SCOPES = [
  'device:calibration',
  'device:checkin',
  'device:conformance',
  'device:desired',
  'device:episodes',
  'device:evaluations',
  'device:incidents',
  'device:releases',
] as const

/** Where a device key lives (§10). */
export const KEY_BINDINGS = ['hardware', 'software'] as const

export type KeyBinding = (typeof KEY_BINDINGS)[number]

/** The credential this robot holds, and the key it was bound to. */
export interface Identity {
  /** The Hub's identifier for this device. */
  device_id?: string | null
  /** The namespace pairing bound this robot to (§10). */
  namespace?: string | null
  /** Lower-case hex of the device public key. */
  device_key?: string | null
  /**
   * Where the private half lives.
   *
   * Absent is "nobody looked" and is reported as such. It is not `software`: a robot that has not
   * been asked whether it has a secure element has not established that it lacks one, and §10's
   * `security.identity.attested` is a measurement rather than an assumption.
   */
  binding?: KeyBinding | null
  /** Whether a credential is held at all. */
  credential?: boolean
  /** The credential's prefix, when it has one. Never the credential. */
  credential_prefix?: string | null
  /**
   * The scopes the credential declares, when it declares any.
   *
   * `null` is an opaque bearer token whose scopes live only on the Hub, which is the ordinary case
   * and is not a fault — see `scope_undeclared`.
   */
  scopes?: string[] | null
}

/** What the robot can observe about itself, from §50's certificate and from the hardware. */
export interface Observed {
  /**
   * The device public key the vendor attested at manufacture (§50.2), when there is a certificate
   * and it verified.
   */
  birth_key?: string | null
  /** Whether the certificate was present and trusted at all. */
  birth_certificate?: boolean
}

export const IDENTITY_FINDINGS = [
  'not_paired',
  'namespace_missing',
  'not_a_device_credential',
  'scope_unrecognised',
  'scope_undeclared',
  'key_binding_unknown',
  'key_not_hardware_bound',
  'key_not_attested',
  'no_birth_certificate',
] as const

export type IdentityFindingCode = (typeof IDENTITY_FINDINGS)[number]

export interface IdentityFinding {
  code: IdentityFindingCode
  severity: 'error' | 'warning'
  message: string
  subjects: string[]
}

export interface IdentityVerdict {
  /** Whether this robot holds a device credential and knows whose it is. */
  paired: boolean
  /**
   * §10's `security.identity.attested`, and it takes **both** halves.
   *
   * The key must live in hardware *and* be the key the vendor signed. Either alone is not the
   * property: a hardware key nobody attested is a key the robot generated for itself and can say
   * nothing about what the machine is, and an attested key sitting in a file is an attestation that
   * copies to any machine somebody puts the file on.
   */
  attested: boolean
  findings: IdentityFinding[]
}

function finding(
  code: IdentityFindingCode,
  severity: 'error' | 'warning',
  message: string,
  subjects: string[] = [],
): IdentityFinding {
  return { code, severity, message, subjects }
}

/**
 * What state this robot's identity is in, and everything wrong with it.
 *
 * Every check runs and the result carries all of them, for `admitPackage`'s reason: an operator
 * standing in front of a robot fixing one problem per attempt is the failure a finding list exists
 * to prevent, and pairing is a job somebody drove to a warehouse to do.
 *
 * **Not paired is not a fault.** §50.2 ships a robot with a birth certificate and no namespace —
 * "it belongs to nobody yet" — so an unpaired robot is a robot at the correct point in its
 * lifecycle, and it is reported as a warning naming the command rather than as something broken.
 */
export function checkIdentity(identity: Identity, observed: Observed): IdentityVerdict {
  const findings: IdentityFinding[] = []
  const credential = identity.credential === true
  const namespace = (identity.namespace ?? '').trim()

  if (!credential) {
    findings.push(finding('not_paired', 'warning',
      'This robot holds no device credential, so it belongs to nobody and reports to nothing. That '
      + 'is the state a robot ships in (§50.2). Pair it with `skynet pair` and the code the Hub '
      + 'showed you.'))
  } else {
    if (namespace === '') {
      findings.push(finding('namespace_missing', 'error',
        'This robot holds a credential and names no namespace. §10 pairs a robot to a namespace, '
        + 'so a credential without one has bound it to nothing — and every report it sends is '
        + 'addressed to a fleet nobody can name.'))
    }

    const prefix = identity.credential_prefix ?? ''
    if (prefix !== DEVICE_TOKEN_PREFIX) {
      findings.push(finding('not_a_device_credential', 'error',
        `A device credential begins \`${DEVICE_TOKEN_PREFIX}\` and this one begins `
        + `${prefix === '' ? 'with nothing recognisable' : `\`${prefix}\``}. §10 puts the narrowest `
        + 'credential class in the system on the robot on purpose, because a credential physically '
        + 'present in the field is the easiest one to lose. A wider one here is a wider one lost.'))
    }

    const scopes = identity.scopes ?? null
    if (scopes === null) {
      findings.push(finding('scope_undeclared', 'warning',
        'This credential declares no scopes, so nothing on the robot can check that it is as '
        + 'narrow as §19 requires. That is the ordinary shape of an opaque bearer token and is not '
        + 'a fault — it means `security.credential.scope` is answered on the Hub and not here.'))
    } else {
      const unrecognised = [...new Set(
        scopes.filter(scope => !(DEVICE_SCOPES as readonly string[]).includes(scope)))].sort()
      if (unrecognised.length > 0) {
        findings.push(finding('scope_unrecognised', 'error',
          `This credential carries ${unrecognised.join(', ')}, which is not a capability a device `
          + 'credential is recognised to need. §19 assumes this credential is lost, and a '
          + 'capability nobody has established is narrow, on a credential everybody assumes is '
          + 'stolen, is refused rather than allowed.',
          unrecognised))
      }
    }
  }

  // ── §10's second addition: what the key is bound to ────────────────────────
  const key = (identity.device_key ?? '').trim()
  const binding = identity.binding ?? null
  const birthKey = (observed.birth_key ?? '').trim()

  if (binding === null) {
    findings.push(finding('key_binding_unknown', 'warning',
      'Nothing has established whether this key lives in a secure element. '
      + '`security.identity.attested` is a measurement, so an unmeasured robot is reported '
      + 'unmeasured rather than counted as either answer.'))
  } else if (binding !== 'hardware') {
    findings.push(finding('key_not_hardware_bound', 'warning',
      'This device key is a file rather than a secret inside a secure element, so it leaves with a '
      + 'copy of the filesystem and identifies whatever machine that copy is put on. That is the '
      + 'ordinary state at guest tier and it is the ceiling §10 puts on '
      + '`security.identity.attested`.'))
  }

  if (observed.birth_certificate !== true) {
    findings.push(finding('no_birth_certificate', 'warning',
      'This robot holds no trusted birth certificate, so there is nothing to check the device key '
      + 'against. §50.3: without one, what this machine is remains a claim it makes about itself.'))
  } else if (birthKey !== '' && key !== '' && birthKey !== key) {
    findings.push(finding('key_not_attested', 'error',
      'The key this robot was paired on is not the key its vendor attested at manufacture. §50.2 '
      + 'separates what it is from whose it is, and this is the seam between them coming apart: '
      + 'either the mainboard was replaced — §50.4, expected, and the remedy is a vendor-signed '
      + 're-issue — or this robot was paired on a key nobody signed.',
      [birthKey]))
  }

  const attested = binding === 'hardware'
    && observed.birth_certificate === true
    && birthKey !== '' && key !== '' && birthKey === key

  return { paired: credential && namespace !== '', attested, findings }
}

// ── §10's third addition: image attestation ──────────────────────────────────

/**
 * The three states one of §10's digests can be in.
 *
 * `nothing` and `unmeasured` are kept apart for §37.1's reason, applied to a measurement rather
 * than to a signature: "there is no policy running" and "a policy is running and its digest could
 * not be read" are two facts with two different people at the end of them, and collapsing them into
 * one missing field would report a robot running unknown software as a robot running none.
 */
export const MEASURE_STATES = ['known', 'nothing', 'unmeasured'] as const

export type MeasureState = (typeof MEASURE_STATES)[number]

/** One of the three digests §10 requires at check-in. */
export interface Measure {
  state: MeasureState
  /** `sha256:…`, when the state is `known`. */
  digest?: string | null
  /** Why it could not be read, when the state is `unmeasured`. */
  why?: string | null
}

/** What is running, as §10 asks it to be reported. */
export interface Attestation {
  /** The Hub's identifier for this robot, when it has one. */
  device_id?: string | null
  os_image: Measure
  policy: Measure
  embodiment: Measure
}

export interface AttestationReport {
  /** Whether every one of the three was measured. */
  complete: boolean
  /** §10's sentence, with every hole in it rendered as the hole it is. */
  sentence: string
  /** The names of the digests that could not be read, sorted. */
  unmeasured: string[]
}

/**
 * One clause of §10's sentence, **including its own label**.
 *
 * The label belongs to the clause rather than to the template, and that is not a stylistic choice:
 * a template reading `running image ${…}` produces "running image `sha256:aa`" for a measured image
 * and "running image an unmeasured OS image" for one that is not. The first version of this did
 * exactly that, and it was found by opening the console rather than by any assertion here — the
 * bytes were correct, the shape was correct, and the sentence was unreadable.
 *
 * So `known` carries the label and the other two do not, which is how English works: *an unmeasured
 * OS image* already says what it is.
 */
function render(measure: Measure, label: string, noun: string): string {
  switch (measure.state) {
    case 'known': {
      const digest = (measure.digest ?? '').trim()
      // A `known` measure with nothing in it is a caller bug, and it is rendered as the absence it
      // is rather than as an empty pair of backticks somebody reads as a digest of zero length.
      return digest === '' ? `an unnamed ${noun}` : `${label}\`${digest}\``
    }
    case 'nothing':
      return `no ${noun}`
    default: {
      const why = (measure.why ?? '').trim()
      return why === '' ? `an unmeasured ${noun}` : `an unmeasured ${noun} (${why})`
    }
  }
}

/**
 * §10's sentence: "robot X is running image sha256:…, policy sha256:…, against body sha256:…".
 *
 * # This function cannot fail, and that is the design
 *
 * There is no `null` return and no error. §13's rule for a recording with a hole in it — seal it,
 * and say it is incomplete — is the same rule, because the reason is the same: the report most
 * likely to have a hole in it is the one covering whatever made the hole. A check-in withheld
 * because a digest could not be read is a robot the fleet believes is **offline**, at the exact
 * moment somebody needs to know what it is running, and it converts a named gap into an
 * indistinguishable silence.
 *
 * So an attestation with three holes in it is still a sentence, and the sentence says so three
 * times.
 *
 * # `nothing` is a real answer for all three
 *
 * Each has a case, and none of them is a stretch. A guest-tier robot runs "a container beside the
 * vendor stack" (§1) and has no Skynet OS image. A robot between policies is running none. A robot
 * whose driver has not come up has no body — §27's `kernel: down` — and reporting a body digest for
 * it would attest joint ratings nothing is applying.
 */
export function attest(attestation: Attestation): AttestationReport {
  const unmeasured: string[] = []
  if (attestation.os_image.state === 'unmeasured') unmeasured.push('os_image')
  if (attestation.policy.state === 'unmeasured') unmeasured.push('policy')
  if (attestation.embodiment.state === 'unmeasured') unmeasured.push('embodiment')
  unmeasured.sort()

  const device = (attestation.device_id ?? '').trim()
  const subject = device === '' ? 'This robot' : device

  const sentence = `${subject} is running ${render(attestation.os_image, 'image ', 'OS image')}, `
    + `${render(attestation.policy, 'policy ', 'policy')}, `
    + `against ${render(attestation.embodiment, 'body ', 'body')}.`

  return { complete: unmeasured.length === 0, sentence, unmeasured }
}
