/**
 * The `.sky` package — what installs onto a robot.
 *
 * Android's second invention, after the vendor interface, was the APK: one artifact that
 * declares what it needs, is signed by whoever built it, and is admitted onto any conformant
 * device by a system that understands the declaration. Before that, software was ported to
 * devices. After it, software was *installed* on them. Robotics is still in the porting era —
 * a policy arrives as a git repository, a conda environment and a README, and getting it onto a
 * second robot is a project rather than a command.
 *
 * A `.sky` package is the Skynet answer, and it deliberately reuses everything the Hub already
 * does rather than inventing a parallel format:
 *
 *   * content is **already content-addressed** — a package is a version's files by sha256, so
 *     installing one on twenty robots transfers each blob once per robot and never re-resolves
 *     what "latest" meant;
 *   * integrity is the version **digest** the Hub already computes;
 *   * authenticity is the **Ed25519 release signature** the Hub already verifies and refuses to
 *     store unverified;
 *   * the trust cost of loading is the **scan verdict** the Hub already derived from the bytes.
 *
 * What is new here is the declaration: capabilities, embodiment requirements, a resource budget
 * and the permissions the package asks the operator to grant.
 *
 * ── Why the budget is mandatory ─────────────────────────────────────────────
 *
 * A phone that runs out of memory kills an app. A robot that runs out of memory mid-stride
 * falls over. Every package declares what it needs and the OS refuses to admit one it cannot
 * feed, rather than admitting it and discovering the shortfall at 60 kg of angular momentum.
 * This is the one place the package format is stricter than an APK, and the reason is physics.
 */

import type { ControlMode } from './embodiment.ts'

export const PACKAGE_KINDS = ['policy', 'skill', 'driver', 'service', 'embodiment'] as const
export type PackageKind = (typeof PACKAGE_KINDS)[number]

/**
 * How a package is isolated.
 *
 * `driver` is the only kind that touches hardware, and it is the only one that may be `native`.
 * A policy is always sandboxed, without exception and without an override flag, because the
 * scanner tells us most policies execute code on load and the whole safety argument rests on
 * them being unable to reach an actuator. An escape hatch here would quietly undo the invariant
 * the Safety Kernel is built to hold.
 */
export const ISOLATION_MODES = ['sandboxed', 'native'] as const
export type IsolationMode = (typeof ISOLATION_MODES)[number]

export interface ResourceBudget {
  ram_mb: number
  vram_mb?: number
  /** Sustained fraction of one core, 1.0 = a full core. */
  cpu_cores?: number
  disk_mb?: number
  /** Rate the package intends to run its loop at. */
  control_hz?: number
  /**
   * Worst-case time from observation in to action out, in milliseconds. Declared, then measured
   * by the conformance suite and compared. A package that overruns its own declaration on real
   * hardware is a package the OS can refuse before it is trusted with a control loop.
   */
  inference_budget_ms?: number
}

export interface EmbodimentRequirement {
  /** Capability tokens from the Standard Robot Interface the package needs. */
  capabilities: string[]
  /** Vendor-specific extension tokens, declared rather than reached for silently. */
  extensions?: string[]
  control_mode: ControlMode
  /** Minimum degrees of freedom, when the policy genuinely needs them. */
  min_dof?: number
  /** Base kinds this package can drive. Empty means any. */
  base_kinds?: string[]
  /** Robot slugs the publisher has actually verified, not the ones they hope work. */
  verified_on?: string[]
}

export interface PackageManifest {
  schema_version: 1
  /** `namespace/name`, matching the Hub repository it came from. */
  id: string
  version: string
  kind: PackageKind
  /** Version digest from the Hub. The identity that matters; the version string is a label. */
  digest: string
  isolation: IsolationMode
  budget: ResourceBudget
  embodiment: EmbodimentRequirement
  /**
   * Capability tokens the package asks the operator to grant. Always a subset of
   * `embodiment.capabilities` — a package cannot request a permission for something it did not
   * declare it needs, which stops "grant everything, use some" from being the path of least
   * resistance.
   */
  requests: string[]
  /** Loader contract from the Hub's load plan, carried so an offline robot need not re-derive it. */
  runtime?: {
    loader?: string
    entrypoint?: string
    config?: string
    container_image?: string
  }
  /** Packages that must be installed first, by id and semver range. */
  depends_on?: Array<{ id: string; range: string }>
  /** Minimum OS version. */
  min_os_version?: string
}

export type PackageRejectionCode =
  | 'unsigned'
  | 'digest_mismatch'
  | 'isolation_violation'
  | 'budget_exceeded'
  | 'capability_ungranted'
  | 'capability_denied'
  | 'over_request'
  | 'missing_dependency'
  | 'os_too_old'
  | 'transport_refused'
  | 'executes_on_load_untrusted'

export interface PackageFinding {
  code: PackageRejectionCode
  /** `error` refuses the install; `warning` proceeds and is shown to the operator. */
  severity: 'error' | 'warning'
  message: string
  subjects?: string[]
}

export interface DeviceCapacity {
  ram_mb: number
  vram_mb?: number
  cpu_cores?: number
  disk_mb?: number
  os_version: string
  /** Capability tokens the robot plus its adapter actually provide. */
  capabilities: string[]
  extensions?: string[]
  /** Grants the operator has made on this robot, by capability token. */
  granted: string[]
  /** Tokens the operator profile withholds outright, whatever a package requests. */
  denied?: string[]
  installed: Array<{ id: string; version: string }>
}

export interface AdmissionParams {
  manifest: PackageManifest
  capacity: DeviceCapacity
  /** Whether a release signature verified against a key registered to the publisher. */
  signature_verified: boolean
  /** The digest the bytes on disk actually hash to. */
  observed_digest: string
  /** Verdict from the transport plan for this body. */
  transport_verdict?: 'direct' | 'retargeted' | 'degraded' | 'refused'
  /** Whether any file in the package executes code when read. */
  executes_on_load?: boolean
  /** Whether the operator explicitly accepted that. Never defaulted true. */
  trust_remote_code?: boolean
}

export interface AdmissionResult {
  admitted: boolean
  findings: PackageFinding[]
  /** Capabilities that will be active: requested, provided by the body, and granted. */
  effective_grants: string[]
}

/**
 * Decide whether a package may be installed.
 *
 * Pure, and shared: the Hub runs it to tell an operator what will happen before they deploy, and
 * the OS runs it at install time on the robot, offline. Neither is authoritative over the other
 * because both compute the same function over the same declared facts.
 *
 * Every check returns a finding rather than throwing, so an operator sees *all* the reasons an
 * install failed instead of the first one. Fixing four problems one deployment at a time is how
 * people end up granting everything.
 */
export function admitPackage(params: AdmissionParams): AdmissionResult {
  const { manifest, capacity } = params
  const findings: PackageFinding[] = []

  if (!params.signature_verified) {
    findings.push({
      code: 'unsigned',
      severity: 'error',
      message: 'No verified publisher signature. The bytes may be intact and nobody has attested to who produced them.',
    })
  }

  if (params.observed_digest !== manifest.digest) {
    findings.push({
      code: 'digest_mismatch',
      severity: 'error',
      message: 'The package contents do not hash to the digest in its manifest.',
      subjects: [manifest.digest, params.observed_digest],
    })
  }

  if (manifest.kind !== 'driver' && manifest.isolation === 'native') {
    findings.push({
      code: 'isolation_violation',
      severity: 'error',
      message: `A ${manifest.kind} package may not run unsandboxed. Only drivers touch hardware directly.`,
    })
  }

  findings.push(...checkBudget(manifest.budget, capacity))

  // A package must not request more than it declared it needs.
  const overRequested = manifest.requests.filter(token => !manifest.embodiment.capabilities.includes(token))
  if (overRequested.length > 0) {
    findings.push({
      code: 'over_request',
      severity: 'error',
      message: 'The package requests permissions it never declared a need for.',
      subjects: overRequested,
    })
  }

  const denied = capacity.denied ?? []
  const explicitlyDenied = manifest.requests.filter(token => denied.includes(token))
  if (explicitlyDenied.length > 0) {
    findings.push({
      code: 'capability_denied',
      severity: 'error',
      message: 'The operator profile withholds capabilities this package requires.',
      subjects: explicitlyDenied,
    })
  }

  const provided = new Set([...capacity.capabilities, ...(capacity.extensions ?? [])])
  const required = [...manifest.embodiment.capabilities, ...(manifest.embodiment.extensions ?? [])]
  const unmet = required.filter(token => !provided.has(token))
  if (unmet.length > 0) {
    findings.push({
      code: 'capability_ungranted',
      severity: 'error',
      message: 'This robot does not provide capabilities the package requires.',
      subjects: unmet,
    })
  }

  const ungranted = manifest.requests.filter(
    token => provided.has(token) && !capacity.granted.includes(token) && !denied.includes(token),
  )
  if (ungranted.length > 0) {
    // A warning, not an error: this is the Android runtime-permission moment. The install
    // proceeds and the operator is prompted; the package simply cannot use the capability until
    // they say yes, and the mode gate enforces that at every cycle.
    findings.push({
      code: 'capability_ungranted',
      severity: 'warning',
      message: 'These capabilities are awaiting an operator grant. The package installs and cannot use them until it is given.',
      subjects: ungranted,
    })
  }

  for (const dependency of manifest.depends_on ?? []) {
    if (!capacity.installed.some(installed => installed.id === dependency.id)) {
      findings.push({
        code: 'missing_dependency',
        severity: 'error',
        message: `Requires ${dependency.id} ${dependency.range}, which is not installed.`,
        subjects: [dependency.id],
      })
    }
  }

  if (manifest.min_os_version && compareVersions(capacity.os_version, manifest.min_os_version) < 0) {
    findings.push({
      code: 'os_too_old',
      severity: 'error',
      message: `Requires Skynet OS ${manifest.min_os_version} or later; this robot runs ${capacity.os_version}.`,
    })
  }

  if (params.transport_verdict === 'refused') {
    findings.push({
      code: 'transport_refused',
      severity: 'error',
      message: 'The policy\'s action space cannot be transported onto this body.',
    })
  }

  if (params.executes_on_load && !params.trust_remote_code) {
    findings.push({
      code: 'executes_on_load_untrusted',
      severity: 'error',
      message:
        'This package contains files that execute code when read, and the operator has not accepted that. ' +
        'Install with trust_remote_code once you have decided the publisher is trustworthy.',
    })
  }

  const admitted = !findings.some(finding => finding.severity === 'error')
  const effective_grants = admitted
    ? manifest.requests.filter(token => provided.has(token) && capacity.granted.includes(token))
    : []

  return { admitted, findings, effective_grants }
}

function checkBudget(budget: ResourceBudget, capacity: DeviceCapacity): PackageFinding[] {
  const shortfalls: string[] = []
  if (budget.ram_mb > capacity.ram_mb) shortfalls.push(`RAM: needs ${budget.ram_mb} MB, has ${capacity.ram_mb} MB`)
  if (budget.vram_mb !== undefined && budget.vram_mb > (capacity.vram_mb ?? 0)) {
    shortfalls.push(`VRAM: needs ${budget.vram_mb} MB, has ${capacity.vram_mb ?? 0} MB`)
  }
  if (budget.disk_mb !== undefined && budget.disk_mb > (capacity.disk_mb ?? 0)) {
    shortfalls.push(`Disk: needs ${budget.disk_mb} MB, has ${capacity.disk_mb ?? 0} MB`)
  }
  if (budget.cpu_cores !== undefined && budget.cpu_cores > (capacity.cpu_cores ?? 0)) {
    shortfalls.push(`CPU: needs ${budget.cpu_cores} cores, has ${capacity.cpu_cores ?? 0}`)
  }

  if (shortfalls.length === 0) return []
  return [{
    code: 'budget_exceeded',
    severity: 'error',
    message: 'This robot cannot supply the resources the package declares it needs.',
    subjects: shortfalls,
  }]
}

/** Numeric-segment comparison, sufficient for `major.minor.patch` OS versions. */
function compareVersions(a: string, b: string): number {
  const left = a.split('.').map(Number)
  const right = b.split('.').map(Number)
  for (let i = 0; i < Math.max(left.length, right.length); i += 1) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0)
    if (diff !== 0) return diff < 0 ? -1 : 1
  }
  return 0
}
