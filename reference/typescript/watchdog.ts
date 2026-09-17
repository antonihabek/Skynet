/**
 * The watchdog chain (§46).
 *
 * Four layers, each catching a failure the one above it cannot see:
 *
 * | # | Watchdog | Catches | Response | Tier |
 * |---|---|---|---|---|
 * | 1 | Kernel `watchdog` guard (§7.1) | The policy stopped producing commands | `stop_cat2` | All |
 * | 2 | systemd `WatchdogSec` on `skynetd` | The supervisor's reconcile loop hung | Restart `skynetd` | All |
 * | 3 | SoC hardware WDT | The application processor wedged entirely | **Reset the SoC** | Host, Certified |
 * | 4 | Safety MCU independent timer | The AP wedged *and* the reset did not recover it | **Cut drive enable** | Certified |
 *
 * Layer 3 is petted by the **Safety Kernel**, not by `skynetd`, and that is what makes it mean
 * anything: the WDT is satisfied only while the thing that actually enforces the envelope is
 * running its cycle. A watchdog petted by a supervisor tells you the supervisor is alive, which is
 * not the question anybody is asking.
 *
 * ── A reset is not a stop ──────────────────────────────────────────────────
 *
 * §46.2's uncomfortable part. When the SoC resets, the drives lose their command source, and what
 * they then do is a property of the **hardware**. Some hold. Some engage brakes. Some fault and
 * disable. **Some coast** — and on such a body, layer 3's own recovery path is a fall.
 *
 * So `hostBlockedByCommandLoss` refuses the host tier for a coasting body, and the shape of that
 * refusal is unusual and correct: **the middle rung is the unsafe one.** Guest is fine —
 * enforcement is advisory and nothing relies on a hardware watchdog. Certified is fine — layer 4
 * cuts drive enable independently of the processor being reset. Only host, where the OS owns the
 * control loop and layer 3 is the last line, is the configuration where a reset means a fall.
 *
 * ── Window nesting (§46.3) ─────────────────────────────────────────────────
 *
 * ```text
 * kernel cycle worst case  <  envelope.watchdog_ms  <  SoC WDT window  <  drive command timeout
 * ```
 *
 * Getting the order wrong makes an upper layer unreachable, and the two ways it goes wrong are
 * different bugs with different remedies — the processor resetting before the kernel can stop, and
 * the drive faulting before the watchdog is ever reached.
 *
 * ── One value per body was not enough (§87.3) ──────────────────────────────
 *
 * §53 carried the gap from the moment Part V was written: *"a humanoid may brake on the legs and
 * coast on the arms. The field is currently one value per body and probably needs to be per
 * chain."* §87.3 makes it per chain, keyed by §5.1's vocabulary, and the decision it settles is
 * what a **missing** chain means — refused, not inherited, because inheriting a body-wide value
 * makes a partially declared humanoid look fully declared.
 *
 * [`resolveCommandLoss`] is the one place that answers *"may this body attempt host"*, whichever
 * way the body declared, and [`hostBlockedByCommandLoss`] stays what it was: the body-wide rules,
 * called by the resolver rather than duplicated beside it.
 */

/** §46.2's vocabulary, in the order the specification lists it. */
export const COMMAND_LOSS_BEHAVIOURS = ['hold', 'brake', 'fault', 'coast'] as const
export type CommandLossBehaviour = (typeof COMMAND_LOSS_BEHAVIOURS)[number]

export interface WatchdogLayer {
  layer: number
  name: string
  catches: string
  response: string
  required_from: string
  petted_by: string
}

/** The chain, as data (§46.1). */
export const CHAIN: readonly WatchdogLayer[] = [
  {
    layer: 1, name: 'kernel_guard',
    catches: 'the policy stopped producing commands',
    response: 'stop_cat2', required_from: 'guest',
    petted_by: 'the policy runtime, every cycle',
  },
  {
    layer: 2, name: 'supervisor',
    catches: "the supervisor's reconcile loop hung",
    response: 'restart skynetd', required_from: 'guest',
    petted_by: 'skynetd, from inside the reconcile loop',
  },
  {
    layer: 3, name: 'soc_wdt',
    catches: 'the application processor wedged entirely',
    response: 'reset the SoC', required_from: 'host',
    // §46.1's sentence, and the reason layer 3 means anything.
    petted_by: 'the Safety Kernel, not skynetd',
  },
  {
    layer: 4, name: 'safety_mcu',
    catches: 'the AP wedged and the reset did not recover it',
    response: 'cut drive enable', required_from: 'certified',
    petted_by: 'an independent timer on the safety MCU',
  },
]

/**
 * Whether a body may attempt the host tier, given what it says its drives do (§46.2).
 *
 * Returns the reason it may not, or `null` when it may.
 *
 * Two refusals, and they are the same refusal: **the recovery path has not been established to be
 * survivable.** A body that coasts has one that demonstrably is not; a body that has declared
 * nothing has one nobody has characterised, and treating silence as `hold` would be inventing the
 * most convenient answer to the only question layer 3 turns on.
 */
export function hostBlockedByCommandLoss(onCommandLoss: string | null | undefined): string | null {
  if (onCommandLoss === 'coast') {
    return "this body's drives coast when commands stop, and layer 3 of the watchdog chain "
      + 'recovers by resetting the processor — which on this machine is a fall. The sound '
      + 'configuration is the certified tier, where layer 4 cuts drive enable independently of '
      + 'the processor (§46.2)'
  }
  if (onCommandLoss === null || onCommandLoss === undefined) {
    return 'this body does not declare what its drives do when commands stop, and layer 3 of the '
      + 'watchdog chain recovers by resetting the processor. Silence is not `hold`: it is a '
      + 'recovery path nobody has characterised (§46.2)'
  }
  if (!(COMMAND_LOSS_BEHAVIOURS as readonly string[]).includes(onCommandLoss)) {
    return `this body declares \`on_command_loss: ${onCommandLoss}\`, which is not one of hold, `
      + 'brake, fault or coast — so what it does while its processor resets is not something '
      + 'anybody can read off the descriptor (§46.2)'
  }
  return null
}

// ── Per chain (§87.3) ────────────────────────────────────────────────────────

/**
 * What one body declares about command loss, from either side of §87.3.
 *
 * `chains` is passed **as data** rather than read off an `EmbodimentDescriptor`, and that is a
 * deliberate refusal to import one: the descriptor is the largest module in the reference edition
 * and this file is four layers and two rules. `chainsOf` in `embodiment.ts` is the other half.
 */
export interface CommandLossParams {
  /** §46.2's field: one behaviour for the whole body. */
  body?: string | null
  /** §87.3's field: one behaviour per §5.1 chain. */
  by_chain?: Record<string, string> | null
  /** The chains this body's joints are on, in §5.1's vocabulary order. */
  chains?: readonly string[]
}

export interface CommandLossReport {
  /** The body-wide declaration, when that is how this body declares. */
  body: string | null
  /** Whether a map was declared at all — which is not the same as a map that covers anything. */
  per_chain: boolean
  /**
   * The behaviour of each chain this body has, in the order `chains` was given.
   *
   * An array rather than an object, for `shapeWatchdog`'s reason one field along: canonical JSON
   * sorts an object's keys, and a chain list read alphabetically puts a body's legs between its
   * hands and its neck. `null` is a chain the body has and the declaration does not cover.
   */
  chains: Array<{ chain: string; behaviour: string | null }>
  /** Chains the declaration names and this body does not have. Over-declaration, not a fault. */
  unknown_chains: string[]
  /** Why host is refused, or null. One answer, whichever way the body declared. */
  host_blocked: string | null
}

/**
 * §87.3's resolution: what each chain does when commands stop, and whether host is available.
 *
 * **One answer about host, whichever way the body declared**, which is why this exists rather than
 * a second `hostBlockedBy…`. §78 is the record of what two spellings of one fact cost, and *"may
 * this body attempt the host tier"* is exactly the kind of fact that acquires a second spelling
 * when a field gains a per-something variant. The body-wide rules stay in
 * [`hostBlockedByCommandLoss`] and this delegates to them.
 *
 * The refusal order is the substance, and two of its steps were found by building it:
 *
 * 1. **Both declared** — `on_command_loss_ambiguous`. §84.2's rule about two action spaces, for the
 *    same reason: two descriptions of one fact, redundant when they agree and a contradiction when
 *    they do not.
 * 2. **A map on a body with no chains** — a body whose joints carry no parseable role is on *no*
 *    chain, so a complete-looking map covers nothing. §5.1 makes a role optional and an
 *    unparseable one is not an error, so this is a real descriptor rather than a broken one, and
 *    the remedy is roles rather than behaviours.
 * 3. **A chain the map does not cover** — refused, not inherited. Inheriting the body-wide value is
 *    the convenient answer and it makes a partially declared humanoid look fully declared.
 * 4. **An unrecognised behaviour, before a `coast`.** `coast` is characterised: §46.2 knows what it
 *    means and the remedy is the certified tier. An unrecognised word is *uncharacterised*, which
 *    is strictly worse and does not rule out coasting either, so it is named first.
 * 5. **`coast` on any chain** — refused, with every coasting chain named. A humanoid whose arms
 *    coast falls with its arms.
 */
export function resolveCommandLoss(params: CommandLossParams): CommandLossReport {
  const chains = params.chains ?? []
  const map = params.by_chain
  const declared = map !== null && map !== undefined
  const body = params.body === undefined ? null : params.body

  const unknown = declared
    ? Object.keys(map).filter(chain => !chains.includes(chain)).sort()
    : []

  const rows = chains.map(chain => ({
    chain,
    behaviour: declared
      ? (typeof map[chain] === 'string' ? map[chain] : null)
      : (typeof body === 'string' ? body : null),
  }))

  const report: CommandLossReport = {
    body,
    per_chain: declared,
    chains: rows,
    unknown_chains: unknown,
    host_blocked: null,
  }

  if (declared && typeof body === 'string') {
    report.host_blocked = 'this body declares both `on_command_loss` and '
      + '`on_command_loss_by_chain`, which are two descriptions of one fact — redundant where they '
      + 'agree and a contradiction where they do not. Delete the one this body does not mean '
      + '(§87.3)'
    return report
  }

  if (!declared) {
    report.host_blocked = hostBlockedByCommandLoss(body)
    return report
  }

  if (chains.length === 0) {
    report.host_blocked = 'this body declares what its drives do per chain, and none of its joints '
      + 'carries a role this vocabulary parses — so the declaration covers no joint at all. §5.1 '
      + 'permits a body the vocabulary does not describe; what it cannot do is declare per chain. '
      + 'Give the joints roles, or declare `on_command_loss` for the body (§87.3)'
    return report
  }

  const uncovered = rows.find(row => row.behaviour === null)
  if (uncovered !== undefined) {
    report.host_blocked = `this body has a ${uncovered.chain} chain and its per-chain declaration `
      + 'does not cover it. A missing chain is refused rather than inheriting a body-wide value, '
      + 'because inheriting makes a partially declared body look fully declared (§87.3)'
    return report
  }

  const unrecognised = rows.find(
    row => !(COMMAND_LOSS_BEHAVIOURS as readonly string[]).includes(row.behaviour as string),
  )
  if (unrecognised !== undefined) {
    report.host_blocked = `this body declares \`${unrecognised.chain}: `
      + `${unrecognised.behaviour}\`, which is not one of hold, brake, fault or coast — so what `
      + 'that chain does while its processor resets is not something anybody can read off the '
      + 'descriptor. An unrecognised behaviour is refused before a `coast` is, because `coast` is '
      + 'characterised and this is not (§46.2, §87.3)'
    return report
  }

  const coasting = rows.filter(row => row.behaviour === 'coast').map(row => row.chain)
  if (coasting.length > 0) {
    report.host_blocked = `this body's ${coasting.join(', ')} drives coast when commands stop, and `
      + 'layer 3 of the watchdog chain recovers by resetting the processor — which for those '
      + 'chains is a fall. The sound configuration is the certified tier, where layer 4 cuts drive '
      + 'enable independently of the processor (§46.2)'
    return report
  }

  return report
}

/** The four windows §46.3 nests, in order, innermost first. */
export const WINDOWS = [
  'kernel_cycle_worst_case_ms', 'envelope_watchdog_ms', 'soc_wdt_ms', 'drive_command_timeout_ms',
] as const
export type WindowName = (typeof WINDOWS)[number]

export interface Windows {
  kernel_cycle_worst_case_ms?: number | null
  envelope_watchdog_ms?: number | null
  soc_wdt_ms?: number | null
  drive_command_timeout_ms?: number | null
}

export interface NestingFault {
  code: string
  inner: WindowName
  outer: WindowName
  detail: string
}

export interface NestingReport {
  faults: NestingFault[]
  missing: WindowName[]
  checked: Array<[WindowName, WindowName]>
  /** Every pair that could be compared nests. **Not** the same as "the chain is sound". */
  ordered: boolean
  /** Every window is declared *and* every pair nests. */
  complete: boolean
}

function nestingCode(inner: string, outer: string): string {
  if (inner === 'kernel_cycle_worst_case_ms' && outer === 'envelope_watchdog_ms') {
    return 'cycle_exceeds_watchdog'
  }
  if (inner === 'envelope_watchdog_ms' && outer === 'soc_wdt_ms') return 'wdt_pre_empts_kernel'
  if (inner === 'soc_wdt_ms' && outer === 'drive_command_timeout_ms') return 'drive_faults_first'
  return 'windows_not_nested'
}

function nestingConsequence(inner: string, outer: string): string {
  if (inner === 'kernel_cycle_worst_case_ms' && outer === 'envelope_watchdog_ms') {
    return 'The kernel cannot complete a cycle inside its own command deadline, so the watchdog '
      + 'guard trips on a robot that is working correctly.'
  }
  if (inner === 'envelope_watchdog_ms' && outer === 'soc_wdt_ms') {
    return 'The processor resets before the kernel executes its controlled stop, so the harsher '
      + 'recovery pre-empts the gentler one and the robot never performs its best available '
      + 'behaviour (§46.3).'
  }
  if (inner === 'soc_wdt_ms' && outer === 'drive_command_timeout_ms') {
    return 'The drive faults on its own before the hardware watchdog fires, so layer 3 is present, '
      + 'configured, and never reached (§46.3).'
  }
  return 'The windows must nest innermost-first for every layer above to be reachable.'
}

function present(value: number | null | undefined): value is number {
  return value !== null && value !== undefined && Number.isFinite(value)
}

/**
 * Check §46.3's nesting.
 *
 * Adjacent pairs only: comparing every pair would report the same inversion up to three times and
 * bury the one an operator has to fix.
 *
 * **Strictly less than.** Two watchdogs with the same window is a race whose winner is decided by
 * interrupt latency, and a chain whose order depends on which timer fires first is not a chain.
 *
 * A gap in the middle does not stop the walk: with no `envelope_watchdog_ms` declared, the kernel
 * cycle and the SoC window are still worth comparing, because that inversion is real whatever sits
 * between them.
 */
export function checkNesting(windows: Windows): NestingReport {
  const ordered: Array<[WindowName, number | null | undefined]> =
    WINDOWS.map(name => [name, windows[name]])

  const missing = ordered.filter(([, value]) => !present(value)).map(([name]) => name)
  const declared = ordered.filter(([, value]) => present(value)) as Array<[WindowName, number]>

  const faults: NestingFault[] = []
  const checked: Array<[WindowName, WindowName]> = []

  for (let index = 0; index + 1 < declared.length; index += 1) {
    const [innerName, inner] = declared[index]
    const [outerName, outer] = declared[index + 1]
    checked.push([innerName, outerName])
    if (inner < outer) continue
    faults.push({
      code: nestingCode(innerName, outerName),
      inner: innerName,
      outer: outerName,
      detail: `${innerName} is ${inner} ms and ${outerName} is ${outer} ms, so the outer window is `
        + `not outside the inner one. ${nestingConsequence(innerName, outerName)}`,
    })
  }

  return {
    faults,
    missing,
    checked,
    ordered: faults.length === 0,
    complete: faults.length === 0 && missing.length === 0,
  }
}
