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
