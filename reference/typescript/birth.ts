/**
 * The birth certificate (§50).
 *
 * §50.3 says what its absence costs, and it is the sentence this module exists to make false:
 *
 * > Without it, *"this is a Unitree G1 with these joint ratings"* is a claim the robot makes about
 * > itself, and the entire safety envelope rests on believing it.
 *
 * A birth certificate is signed **at manufacture**, by the vendor, over a device public key that
 * was generated inside a secure element and never left it. It binds model, serial number, the
 * embodiment descriptor digest, hardware revision, manufacture date, and the factory calibration
 * records.
 *
 * ── The distinction the whole section turns on ──────────────────────────────
 *
 * §50.2: *"the birth certificate proves **what it is**; pairing (§10) binds it to **whose it
 * is**. Two separate facts, established by two separate parties, which is correct — the vendor is
 * the only party who can attest to the first and has no business asserting the second."*
 *
 * So a certificate carrying a namespace, an owner or an operator is not a richer certificate. It
 * is a vendor asserting something they are not a party to, and `check` reports it — as a
 * **finding about the certificate**, not as a failure of the robot, because the machine is fine
 * and the document is overreaching.
 *
 * ── A mismatch is expected behaviour, not a fault ───────────────────────────
 *
 * §50.4: *"replacing a mainboard produces a robot whose birth certificate no longer matches its
 * hardware. That is the correct behaviour and not a bug — it is the same property that makes a
 * phone's secure enclave meaningful."*
 *
 * This is why the findings below are not a boolean. A robot that had a board swapped is not a
 * robot that failed; it is a robot whose identity needs a vendor-signed re-issue, and the finding
 * says that in the sentence rather than leaving somebody to work it out from `valid: false`. The
 * remedies for *"the serial does not match"*, *"the descriptor digest does not match"* and *"the
 * vendor's key is not one this robot trusts"* are three different phone calls.
 *
 * ── Pure, like everything else here ─────────────────────────────────────────
 *
 * No clock, no key, no secure element. The signature state arrives decided, as it does for a
 * package (§37.1) and for an entitlement (§51), and what the robot actually observes about itself
 * arrives as an `Observed`. The Hub checking a fleet's certificates from a check-in and a robot
 * checking its own at boot are then the same function over the same facts.
 */

import type { SignatureState } from './package.ts'

/** What a vendor signed at manufacture (§50.2). */
export interface BirthCertificate {
  /**
   * The device public key, generated inside the secure element. Lower-case hex.
   *
   * The subject of the whole document: everything else binds *to this key*, and the private half
   * never left the element, so a certificate is only evidence about the machine holding it.
   */
  device_key: string
  /** The model, as the vendor names it. */
  model: string
  /** The serial number. */
  serial: string
  /**
   * The digest of the embodiment descriptor this body was measured as.
   *
   * The field that closes §50.3. A robot presenting a descriptor whose digest is not this one is
   * presenting joint ratings nobody attested, and §5's `verified` flag cannot tell you that
   * because a descriptor can be perfectly well signed and be for a different machine.
   */
  descriptor_digest: string
  /** The hardware revision. */
  hardware_revision: string
  /**
   * Manufacture date, as the vendor wrote it. Never parsed here.
   *
   * A string rather than an instant on purpose: it is provenance, not a deadline. Nothing in this
   * module compares it to a clock, and a type that invited it to be compared would invite the
   * certificate to expire — which §50 never says it does, and which would brick a robot in a
   * basement for a reason §51.1 already rejects in the commercial case.
   */
  manufactured: string
  /**
   * Digests of the factory calibration records this certificate covers (§45.4).
   *
   * §50.3: *"it also gives the factory calibration a provenance chain: the extrinsics measured on
   * the line are signed by the party that measured them."*
   */
  calibration_digests: string[]
  /**
   * Anything the certificate says about **whose** the robot is.
   *
   * Should always be empty. Present as a field, rather than silently discarded at parse, so that
   * `check` can report a vendor who asserted it — see `ownership_asserted`.
   */
  claims_ownership: string[]
  /** What is known about the vendor's signature over this certificate. */
  signature: SignatureState
}

/**
 * What the robot can see about itself, right now.
 *
 * Every field is optional because a robot early in boot has seen some of these and not others, and
 * §7.2's rule applies here as everywhere: an absent input makes a check **unavailable**, never
 * passed. An absent value never satisfies a comparison.
 */
export interface Observed {
  /** The public key the secure element actually holds, lower-case hex. */
  device_key?: string | null
  /** The serial the machine reports. */
  serial?: string | null
  /** The digest of the descriptor this robot is running. */
  descriptor_digest?: string | null
  /** The hardware revision the machine reports. */
  hardware_revision?: string | null
}

/**
 * One thing wrong, or one thing that could not be checked.
 *
 * Ordered by how much they cost, and each names its own remedy in `explain` because the remedies
 * are genuinely different: a re-issue, a resync, a trust-store change, or a call to the vendor.
 */
export const BIRTH_FINDINGS = [
  'unattested',
  'device_key_mismatch',
  'serial_mismatch',
  'descriptor_mismatch',
  'hardware_revision_mismatch',
  'ownership_asserted',
  'unobservable',
  'incomplete',
] as const

export type BirthFindingCode = (typeof BIRTH_FINDINGS)[number]

/**
 * A finding, with whatever it is about.
 *
 * `subjects` is uniform and the order within it is load-bearing: for a mismatch it is **what the
 * certificate says, then what the machine reports**, in that order and never the reverse. A
 * reader who has to work out which of two hex strings came from the document is a reader who will
 * get it wrong once.
 *
 * | Code | `subjects` |
 * |---|---|
 * | `unattested` | the signature state |
 * | `*_mismatch` | the certificate's value, then the observed one |
 * | `ownership_asserted` | the fields the vendor asserted |
 * | `unobservable`, `incomplete` | the field, as it is named to a person |
 */
export interface BirthFinding {
  code: BirthFindingCode
  subjects: string[]
}

/**
 * Whether this finding means the certificate establishes nothing at all.
 *
 * True only for `unattested` and a mismatched device key. The others narrow what is attested;
 * these two remove it.
 */
export function voidsTheCertificate(finding: BirthFinding): boolean {
  return finding.code === 'unattested' || finding.code === 'device_key_mismatch'
}

/** The sentence an operator gets, naming the remedy. */
export function explain(finding: BirthFinding): string {
  const [first = '', second = ''] = finding.subjects
  switch (finding.code) {
    case 'unattested':
      return `The birth certificate's signature is ${first}, so it attests nothing: everything it `
        + 'says about this machine is currently a claim the machine makes about itself, which is '
        + 'the situation a birth certificate exists to end. Register the vendor\'s key with this '
        + 'robot\'s trust store.'
    case 'device_key_mismatch':
      return `This certificate was issued over device key ${first} and the secure element holds `
        + `${second}. The document is about a different machine. If the mainboard was replaced `
        + 'this is the expected result and not a fault — §50.4 — and the remedy is a vendor-signed '
        + 're-issue, which also invalidates the calibrations a board swap could have disturbed.'
    case 'serial_mismatch':
      return `The certificate names serial "${first}" and this machine reports "${second}". Ask `
        + 'the vendor to re-issue.'
    case 'descriptor_mismatch':
      return `The certificate attests descriptor ${first} and this robot is running ${second}. `
        + 'The joint ratings in force are ones nobody has signed for this machine. A vendor who '
        + 'has published an updated descriptor should re-issue the certificate over it; until then '
        + 'the envelope rests on the descriptor\'s own signature rather than on the vendor\'s '
        + 'attestation that it belongs to this body.'
    case 'hardware_revision_mismatch':
      return `The certificate names hardware revision "${first}" and this machine reports `
        + `"${second}". Either the machine was modified or the certificate is stale; both are a `
        + 'vendor re-issue.'
    case 'ownership_asserted':
      return `This certificate asserts ownership (${finding.subjects.join(', ')}), and ownership `
        + 'is not the vendor\'s to assert. §50.2 keeps two facts apart: the certificate proves '
        + 'what this robot is, and pairing proves whose it is. The assertion is ignored.'
    case 'unobservable':
      return `The certificate binds ${first} and this robot could not read its own. The binding is `
        + 'neither confirmed nor contradicted — it is unchecked, which is not the same as '
        + 'matching.'
    case 'incomplete':
      return `The certificate does not bind ${first}, which §50.2 requires it to. It is not a `
        + 'complete birth certificate and whatever it does bind is all it attests.'
  }
}

/** What checking one certificate against one machine concluded. */
export interface Attested {
  /** Everything wrong or unchecked, worst first. */
  findings: BirthFinding[]
}

/**
 * Whether this certificate attests anything at all about this machine.
 *
 * False when the signature cannot be believed or the device key is another machine's. Note what
 * this is **not**: it is not "may the robot run". §50 establishes identity; it does not gate
 * motion, and a robot whose certificate is void is a robot with an unattested identity, which is
 * exactly the state every robot was in before §50 existed.
 */
export function attests(result: Attested): boolean {
  return !result.findings.some(voidsTheCertificate)
}

/** Whether every binding the certificate makes was checked and matched. */
export function fullyConfirmed(result: Attested): boolean {
  return result.findings.length === 0
}

/** The bindings that could not be checked, as their field names. */
export function unchecked(result: Attested): string[] {
  return result.findings
    .filter(finding => finding.code === 'unobservable')
    .map(finding => finding.subjects[0] ?? '')
}

function compare(
  certificate: string,
  observed: string | null | undefined,
  field: string,
  mismatch: BirthFindingCode,
  findings: BirthFinding[],
): void {
  if (certificate === '') {
    findings.push({ code: 'incomplete', subjects: [field] })
    return
  }
  if (observed === null || observed === undefined) {
    findings.push({ code: 'unobservable', subjects: [field] })
    return
  }
  if (observed === certificate) return
  findings.push({ code: mismatch, subjects: [certificate, observed] })
}

/**
 * Check a birth certificate against what the robot observes about itself (§50).
 *
 * The order of the findings is the order of their cost, and the first one is deliberately about
 * the *document* rather than the machine: comparing fields against a certificate nobody signed is
 * comparing the robot to a story it told itself.
 */
export function check(certificate: BirthCertificate, observed: Observed): Attested {
  const findings: BirthFinding[] = []

  if (certificate.signature !== 'verified') {
    findings.push({ code: 'unattested', subjects: [certificate.signature] })
  }

  compare(certificate.device_key, observed.device_key, 'the device key',
    'device_key_mismatch', findings)
  compare(certificate.serial, observed.serial, 'a serial number',
    'serial_mismatch', findings)
  compare(certificate.descriptor_digest, observed.descriptor_digest,
    'an embodiment descriptor digest', 'descriptor_mismatch', findings)
  compare(certificate.hardware_revision, observed.hardware_revision, 'a hardware revision',
    'hardware_revision_mismatch', findings)

  if (certificate.model === '') {
    // Bound but not observable: a model is what the vendor calls this design, and there is
    // nothing on the machine to compare it against. Its absence is still incomplete.
    findings.push({ code: 'incomplete', subjects: ['a model'] })
  }

  if (certificate.claims_ownership.length > 0) {
    findings.push({ code: 'ownership_asserted', subjects: [...certificate.claims_ownership] })
  }

  return { findings }
}

/**
 * An empty certificate whose signature state the caller must state.
 *
 * The one argument with no default is the one whose omission would matter, which is the whole
 * reason this is a constructor rather than an object literal somebody copies.
 */
export function unsigned(signature: SignatureState): BirthCertificate {
  return {
    device_key: '',
    model: '',
    serial: '',
    descriptor_digest: '',
    hardware_revision: '',
    manufactured: '',
    calibration_digests: [],
    claims_ownership: [],
    signature,
  }
}

/**
 * The fields §50.2 says a certificate binds, in the order it lists them.
 *
 * Exported because two things need the same list and disagreeing about it is how a field quietly
 * stops being required: the reader that decodes a certificate, and whatever eventually issues one.
 */
export const REQUIRED_FIELDS = [
  'device_key',
  'model',
  'serial',
  'descriptor_digest',
  'hardware_revision',
  'manufactured',
] as const

/**
 * The fields a certificate must never carry (§50.2).
 *
 * Ownership is established by pairing, by a different party. A vendor who put a namespace in a
 * birth certificate would be asserting a fact they cannot know, and the robot ignores it — but it
 * says so, because silently ignoring a field means the next vendor adds it too.
 */
export const OWNERSHIP_FIELDS = ['namespace', 'owner', 'operator', 'customer'] as const
