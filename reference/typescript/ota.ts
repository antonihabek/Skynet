/**
 * §12's three rules, as two decisions.
 *
 * §12 gives OTA three rules that are *"not standard practice"*, and the third is the one the
 * document calls the single most important test in the whole suite:
 *
 * 1. **Never apply mid-task.** An update issued while the robot is holding something is staged and
 *    applied at a declared safe state. `ota.safe_state.respected` measures it.
 * 2. **Health-gated rollback.** The new slot must pass a health check within a boot budget or the
 *    bootloader returns to the previous slot unattended. `ota.rollback.works` measures it.
 * 3. **OTA is not a route around certification.** An update that changes the Safety Kernel or the
 *    envelope is refused without a fresh bench pass. *"Without this rule, every guarantee in §7 and
 *    §15 is one silent update away from being void."*
 *
 * **Rule 1 is not here**, and that is worth saying rather than leaving as an absence: it is already
 * enforced, structurally, by the reconciler — staging is an open action and applying is gated on a
 * safe state, so an apply cannot be emitted for a robot that is not at one. A second implementation
 * of it here would be a second answer to a question already answered.
 *
 * Rules 2 and 3 are decisions, and decisions are functions. `admitBundle` answers rule 3 and
 * `confirmBoot` answers rule 2.
 *
 * # Why rule 3 fails closed on a word nobody recognises
 *
 * A bundle declares which components it changes. Two of them — `safety_kernel` and `envelope` —
 * require a bench pass for **these exact bytes** before the update may be applied.
 *
 * An unrecognised component name is treated as though it were one of those two. That is the whole
 * anti-gaming argument in one rule: a publisher who wanted to ship a kernel change past the gate
 * would not write `safety_kernel`, they would write `core` or `rt` or `platform` — and a matcher
 * that only knew two words would wave it through. The failure mode of the strict reading is an
 * update that waits for somebody to fix a typo. The failure mode of the lenient reading is §7's
 * guarantees quietly voided, which is the sentence §12 spends its length on.
 *
 * # And why a bench pass is bound to a digest
 *
 * The same reason §56.3's seeds come from the benchmark: evidence that is not bound to what it is
 * evidence *of* is not evidence. A bench pass for version 1.4.0 says nothing about 1.4.1, and a gate
 * that accepted "this robot has passed a bench test at some point" would be satisfied by every robot
 * that had ever been certified — which is every robot the gate exists to protect.
 *
 * # Pure
 *
 * No clock, no key, no filesystem. The signature arrives decided, and the boot budget arrives as
 * elapsed milliseconds rather than as a timestamp (§35.2).
 */

import type { SignatureState } from './package.ts'

/**
 * The components an OS bundle may declare that it changes.
 *
 * Deliberately the stack §3 draws and nothing more. A vocabulary invented wider than the document
 * would be this file deciding what an operating system is made of, and the only two entries that
 * carry a rule are the two §12 names.
 */
export const OS_COMPONENTS = [
  'bootloader',
  'kernel_image',
  'rhal_abi',
  'rhal_driver',
  'safety_kernel',
  'envelope',
  'skynetd',
  'policy_runtime',
  'userspace',
] as const

/**
 * The components §12's third rule gates, by name.
 *
 * **Not the whole rule.** `gated` is, because an unrecognised name is gated too.
 */
export const CERTIFICATION_GATED = ['safety_kernel', 'envelope'] as const

/**
 * Whether changing this component requires a fresh bench pass.
 *
 * True for §12's two names, and true for anything this edition does not recognise. See the header:
 * a component nobody can classify is one nobody has established is safe to change, and the two
 * readings fail in opposite directions.
 */
export function gated(component: string): boolean {
  if ((CERTIFICATION_GATED as readonly string[]).includes(component)) return true
  return !(OS_COMPONENTS as readonly string[]).includes(component)
}

/** What an OS release says about itself (§57's `GET /os/releases`). */
export interface BundleManifest {
  version?: string
  /** `stable`, `beta`, whatever the fleet uses. */
  channel?: string
  /** `sha256:…` over the bundle. What a bench pass is bound to. */
  digest?: string
  /** Machine identifiers this image is built for. */
  compatible?: string[]
  /** Which of `OS_COMPONENTS` differ from the running image. */
  changes?: string[]
}

/** A bench pass, and what it is a pass *of*. */
export interface BenchPass {
  image_digest: string
  tier: string
  /** Whether every required test passed. A recorded run that failed is still a record. */
  passed: boolean
}

/** The robot this bundle would be applied to. */
export interface UpdateSubject {
  running_version?: string
  /**
   * What this machine is, when it knows. Absent omits the compatibility check rather than failing
   * it: a robot that cannot say what board it is has not established a mismatch.
   */
  board?: string | null
  channel?: string
  signature: SignatureState
  bench?: BenchPass | null
}

export const BUNDLE_FAULTS = [
  'unversioned',
  'undigested',
  'signature_failed',
  'signature_untrusted',
  'signature_absent',
  'signature_unverifiable',
  'incompatible',
  'compatibility_undeclared',
  'channel_mismatch',
  'certification_required',
  'bench_pass_stale',
  'bench_pass_failed',
  'already_running',
] as const

export type BundleFaultCode = (typeof BUNDLE_FAULTS)[number]

export interface BundleFault {
  code: BundleFaultCode
  severity: 'error' | 'warning'
  message: string
  subjects: string[]
}

export interface BundleVerdict {
  admitted: boolean
  faults: BundleFault[]
  /**
   * The declared components that require a bench pass, sorted.
   *
   * Reported whether or not one was held, because *"this update touches the safety kernel"* is a
   * fact an operator wants on the screen before they go and book bench time.
   */
  gated_components: string[]
}

function fault(
  code: BundleFaultCode,
  severity: 'error' | 'warning',
  message: string,
  subjects: string[] = [],
): BundleFault {
  return { code, severity, message, subjects }
}

function displayDigest(digest: string): string {
  return digest === '' ? 'nothing' : digest
}

function certificationMessage(gatedComponents: string[], unrecognised: string[]): string {
  let message = `This update changes ${gatedComponents.join(', ')}, and §12 refuses one that `
    + 'changes the safety kernel or the envelope without a fresh bench pass for these exact bytes.'
  if (unrecognised.length > 0) {
    message += ` ${unrecognised.join(', ')} is not a component this edition recognises, and an `
      + 'unrecognised component is gated for the same reason the two named ones are: nobody has '
      + 'established what it changes.'
  }
  message += ' This robot holds no bench pass at all.'
  return message
}

/**
 * §12's third rule, and everything else that decides whether an image may be installed.
 *
 * Every check runs, and the result carries all of them. An operator fixing four problems one attempt
 * at a time is the failure `admitPackage` already refuses to cause, and an OS update is the case
 * where each attempt costs a reboot.
 */
export function admitBundle(bundle: BundleManifest, subject: UpdateSubject): BundleVerdict {
  const faults: BundleFault[] = []
  const version = bundle.version ?? ''
  const digest = bundle.digest ?? ''
  const channel = bundle.channel ?? ''
  const compatible = bundle.compatible ?? []
  const changes = bundle.changes ?? []

  if (version.trim() === '') {
    faults.push(fault('unversioned', 'error',
      'This bundle names no version, so nothing can say what the robot would be running '
      + 'afterwards. Rebuild it with a version.'))
  }

  // Before the certification gate, because that gate binds a bench pass to this digest and a bundle
  // with no digest cannot be bound to anything. Reported separately so the two refusals do not read
  // as one problem.
  if (digest.trim() === '') {
    faults.push(fault('undigested', 'error',
      "This bundle names no digest. §12's certification gate binds a bench pass to the exact bytes "
      + 'it was taken against, and bytes with no name cannot be bound to.'))
  }

  // §37.1's five outcomes, kept apart here for the reason they are kept apart there: four different
  // facts with four different people at the end of them.
  switch (subject.signature) {
    case 'verified': break
    case 'failed':
      faults.push(fault('signature_failed', 'error',
        'A signature was present and did not verify. The bytes are attested to somebody other than '
        + 'the manifest claims, which is what tampering looks like — do not install this, and find '
        + 'out where it came from.'))
      break
    case 'untrusted':
      faults.push(fault('signature_untrusted', 'error',
        'The signature verifies against a key this robot has not been told to believe. Add the key '
        + 'with `skynetd trust add`, or find out why a stranger signed an OS image for this fleet.'))
      break
    case 'absent':
      faults.push(fault('signature_absent', 'error',
        'Nobody signed this bundle. §12 signs images with the same Ed25519 machinery as releases, '
        + 'and an unsigned root filesystem is a root filesystem anybody on the path could have '
        + 'written.'))
      break
    default:
      faults.push(fault('signature_unverifiable', 'error',
        'This platform cannot check a signature at all, so it cannot establish that this image is '
        + "the one the fleet published. That is the operating system's problem rather than the "
        + "publisher's."))
      break
  }

  // Compatibility. A bundle that declares nothing is a **warning**, because §12 does not require the
  // field and refusing on it would stop every image built before it existed — but it is said out
  // loud, because "this image does not know what it is for" is exactly what somebody wants to have
  // been told after flashing the wrong board.
  const board = subject.board ?? null
  if (compatible.length === 0) {
    faults.push(fault('compatibility_undeclared', 'warning',
      'This bundle declares no compatible machines, so nothing here can tell whether it is built '
      + 'for this one.'))
  } else if (board !== null && !compatible.includes(board)) {
    faults.push(fault('incompatible', 'error',
      `This bundle is built for ${compatible.join(', ')}, and this machine is \`${board}\`.`,
      [...compatible]))
  } else if (board === null) {
    // A robot that cannot say what board it is has not established a mismatch, and refusing would
    // make an unknown machine unupdatable — which is the state a robot is in exactly when it most
    // needs an update.
    faults.push(fault('compatibility_undeclared', 'warning',
      "This machine does not report what it is, so the bundle's compatibility with "
      + `${compatible.join(', ')} could not be checked.`,
      [...compatible]))
  }

  const wanted = subject.channel ?? ''
  if (wanted !== '' && channel !== '' && wanted !== channel) {
    faults.push(fault('channel_mismatch', 'error',
      `The desired state asks for the \`${wanted}\` channel and this bundle is \`${channel}\`. A `
      + 'robot pulled onto another channel by an image rather than by an operator is a robot nobody '
      + 'moved.'))
  }

  // ── §12's third rule ───────────────────────────────────────────────────────
  const gatedComponents = [...new Set(changes.filter(component => gated(component)))].sort()

  if (gatedComponents.length > 0) {
    const unrecognised = gatedComponents.filter(
      component => !(OS_COMPONENTS as readonly string[]).includes(component))
    const bench = subject.bench ?? null
    if (bench === null) {
      faults.push(fault('certification_required', 'error',
        certificationMessage(gatedComponents, unrecognised), [...gatedComponents]))
    } else if (bench.image_digest !== digest || digest === '') {
      faults.push(fault('bench_pass_stale', 'error',
        `The bench pass this robot holds was taken against \`${displayDigest(bench.image_digest)}\` `
        + `and this bundle is \`${displayDigest(digest)}\`. Evidence that is not bound to what it `
        + 'is evidence of is not evidence — book a fresh bench pass for these bytes.',
        [...gatedComponents]))
    } else if (!bench.passed) {
      faults.push(fault('bench_pass_failed', 'error',
        'A bench pass exists for these bytes and it records a failure. This update changes '
        + `${gatedComponents.join(', ')}, and §12 refuses one that has not passed rather than one `
        + 'that has not been tried.',
        [...gatedComponents]))
    }
  }

  const running = subject.running_version ?? ''
  if (running !== '' && running === version) {
    faults.push(fault('already_running', 'warning',
      `This robot is already running \`${version}\`. Staging it again is harmless and applying it `
      + 'costs a reboot for nothing.'))
  }

  return {
    admitted: !faults.some(entry => entry.severity === 'error'),
    faults,
    gated_components: gatedComponents,
  }
}

// ── §12's second rule ────────────────────────────────────────────────────────

export const BOOT_DECISIONS = ['not_pending', 'wait', 'confirm', 'roll_back'] as const

export type BootDecision = (typeof BOOT_DECISIONS)[number]

/**
 * §12's default boot budget, in milliseconds.
 *
 * Ten minutes. Long enough for a robot that boots slowly, fetches a desired state over a poor link
 * and starts a driver that probes a bus; short enough that a warehouse at 3 a.m. is not a warehouse
 * with a brick in it until morning. The document does not give a number, so this is a default an
 * operator may narrow rather than a constant anybody derived.
 */
export const DEFAULT_BOOT_BUDGET_MS = 600_000

/** What the supervisor knows about this boot. */
export interface BootEvidence {
  /**
   * Whether this boot is the first into a slot nothing has confirmed.
   *
   * False makes every other field irrelevant, and that is deliberate: a supervisor that could mark
   * a slot bad on an ordinary boot would be a supervisor that can roll back a robot nobody updated.
   */
  pending?: boolean
  elapsed_ms?: number
  budget_ms?: number
  /**
   * Whether §48.2's self-test concluded the robot may run.
   *
   * Absent is *"the self-test has not finished"*, which is not a pass and is not a failure — it is
   * the ordinary state for the first seconds of a boot, and it is why `wait` exists.
   */
  post_passed?: boolean | null
  packages_healthy?: boolean | null
  converged?: boolean | null
}

export interface BootVerdict {
  decision: BootDecision
  detail: string
  /** The named checks that have not passed, sorted. Empty on a confirm. */
  missing: string[]
}

/**
 * §12's second rule — health-gated rollback.
 *
 * > The new slot must pass a health check within a boot budget or the bootloader returns to the
 * > previous slot unattended.
 *
 * Three properties, and each is the reason a rule of this shape is written down rather than left to
 * a script:
 *
 * * **Absence of evidence is not health.** Every check is optional and absent never confirms. A
 *   supervisor that marked a slot good because it had not yet been told otherwise would replace
 *   §12's unattended recovery with a race between the health check and the confirmation.
 * * **The budget is what makes it unattended.** Without it a robot that never becomes healthy waits
 *   for ever, in the state the rollback exists to leave. With it, the worst case is a reboot.
 * * **A robot that is not pending decides nothing.** Marking a slot bad on an ordinary boot is a
 *   rollback of an update nobody made.
 */
export function confirmBoot(evidence: BootEvidence): BootVerdict {
  if (evidence.pending !== true) {
    return {
      decision: 'not_pending',
      detail: 'This boot is not the first into a new slot, so there is nothing to confirm and '
        + 'nothing to roll back.',
      missing: [],
    }
  }

  const elapsed = evidence.elapsed_ms ?? 0
  const budget = evidence.budget_ms ?? DEFAULT_BOOT_BUDGET_MS

  const missing: string[] = []
  if (evidence.post_passed !== true) missing.push('post')
  if (evidence.packages_healthy !== true) missing.push('packages')
  if (evidence.converged !== true) missing.push('converged')
  missing.sort()

  if (missing.length === 0) {
    return {
      decision: 'confirm',
      detail: 'The self-test passed, every wanted package is running and the reconciler converged, '
        + `${elapsed} ms into a ${budget} ms budget. The slot is good.`,
      missing,
    }
  }

  // A budget that is not a positive number is no budget, and no budget means waiting for ever rather
  // than rolling back on the first pass. The safe direction here is the patient one: an unattended
  // rollback triggered by a misconfigured budget is a working robot rebooted into an older image for
  // no reason.
  const expired = Number.isFinite(budget) && budget > 0
    && Number.isFinite(elapsed) && elapsed >= budget

  if (expired) {
    return {
      decision: 'roll_back',
      detail: `${elapsed} ms have passed and the budget is ${budget} ms, `
        + `and ${missing.join(', ')} still ${missing.length === 1 ? 'has' : 'have'} not passed. `
        + 'Marking the slot bad; the bootloader returns to the previous one.',
      missing,
    }
  }

  return {
    decision: 'wait',
    detail: `${elapsed} ms into a ${budget} ms budget, with `
      + `${missing.join(', ')} still to pass. Saying nothing to the bootloader until they do or the `
      + 'budget expires.',
    missing,
  }
}
