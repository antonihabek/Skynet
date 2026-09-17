/**
 * §88 — what admits a motion request on a program-level body.
 *
 * §54.1 ended with two sentences that stood from the moment Part VI was written:
 *
 * > `StandardRobotInterface.execute({ skill, arguments, timeout_s })` already existed on the Hub
 * > side as the invocation point. **Nothing routed it.**
 *
 * For a joint-commanded body that stopped being true once §54.4's lifecycle was driven: the skill's
 * model *is* a policy, the supervisor starts it, and it posts commands into the ring the Safety
 * Kernel reads. For §59.2's machines — a Fanuc, a KUKA, an ABB, an older UR, which between them are
 * most of the world's installed industrial robots — it was still true, and `DRY-COVERAGE-PLAN.md`
 * Phase 4 named two blockers. §84 removed the first by giving a motion a place in the descriptor.
 *
 * ── The second blocker dissolved rather than being bridged ─────────────────
 *
 * As written: §33.1 keeps `skynetd` off the path that holds a driver, so a routed skill's
 * destination is `skynetk`'s and **there is no channel from the supervisor to it**. Both obvious
 * remedies spend §33.1's licence — a socket from the supervisor carrying motions puts it on the
 * actuator path, and moving the driver into the supervisor deletes the section outright.
 *
 * §88.1's answer is that the route already exists. §54.4's `running` row says the skill's policy
 * runs *"exactly like any other policy — a skill is not an exemption from the kernel"*, and §84.2
 * says a policy whose action space is motions is a policy. So a skill over a program-level body is a
 * policy process emitting **motion requests** into the same ring it would post joint commands into;
 * `skynetk` reads them and calls the driver's `extension()`; and the supervisor admits the
 * invocation, evaluates §54.2's preconditions, takes §25.2's leases and starts a unit — every one of
 * which it already does — and never sees a motion.
 *
 * What was actually missing was not a channel but the **admission rule** for a request arriving on
 * one. A joint command is admitted by §7.1's guards; a motion is not a joint command (§84.4), so
 * nothing had decided what `skynetk` should do with one. That is this file.
 *
 * ── It is a gate, and calling it a guard would be a claim ──────────────────
 *
 * §7's guards clamp a *value* against a resolved ceiling. There is no ceiling for `move_l` and no
 * clamp that produces a safer program, because the controller owns the servo loop and its safety
 * case belongs to somebody else — which is §59.2's whole point and §84.4 restates it. What is
 * checkable is **authority, state, and the body's own declared table**, and the last of those is the
 * half §84.1 created and nothing consumed. [`admitMotion`] is not a ninth guard and this file does
 * not pretend it is.
 */

import type { EmbodimentMotion, MotionParameter } from './embodiment.ts'

/**
 * §27's states in which a body will take a command.
 *
 * `running` and `degraded`, and the second is not a softening: §27 puts `DEGRADED` on a body that is
 * *still there and still answering*, and §36.3's whole ladder is about a robot that keeps working
 * while giving things up. `hold`, `unhomed`, `post`, `stopping` and `safe` each mean the body is not
 * in a position to be told to run a program, and a gate that accepted a motion in `stopping` would
 * be queueing work behind a controlled stop.
 */
export const MOTION_READY_STATES = ['running', 'degraded'] as const

/**
 * Why a motion request was refused, in the order [`admitMotion`] evaluates them.
 *
 * Declaration order **is** precedence, which is the convention §84.1's fault list already uses, and
 * it is worth the sentence because the first four are about the robot and the last six are about the
 * request: a caller reading a refusal from the first group has nothing to fix in its own bytes.
 */
export const MOTION_REFUSALS = [
  'no_lease',
  'estop',
  'not_active',
  'busy',
  'unknown_motion',
  'deadline_undeclared',
  'parameter_unknown',
  'parameter_missing',
  'parameter_not_finite',
  'parameter_out_of_bounds',
] as const
export type MotionRefusalCode = (typeof MOTION_REFUSALS)[number]

/** One request, as a policy emits it. */
export interface MotionRequest {
  /**
   * The **body's** own spelling, resolved by the transport plan at install (§84.3).
   *
   * Not the policy's: a `remap` binding is precisely the case where the two differ, and a runtime
   * that sent the policy's name would be asking the controller for a program it does not have on a
   * body the plan said was compatible.
   */
  motion: string
  /** By name, because a program-level controller's arguments are named in its own table. */
  parameters: Record<string, number>
  /**
   * How long the controller may take, milliseconds (§88.3).
   *
   * Required. §7.1's watchdog guard asks whether the policy produced a command *this cycle*, and a
   * motion takes seconds during which a correct policy produces nothing — so the guard has no
   * question to ask on this path and the motion carries its own bound instead. Bounded above by the
   * invocation's own deadline, which is §54.5's rule that the root's timeout bounds the whole tree
   * with a motion as the leaf.
   */
  deadline_ms?: number | null
  /** Who is asking. Compared against the lease, and §25.2 holds leases by holder string. */
  holder?: string | null
}

/** What the process holding the driver knows when a request arrives. */
export interface MotionGate {
  /** The body's declared motions (§84.1), in declaration order. */
  motions: readonly EmbodimentMotion[]
  /** §27's operating state. */
  state: string
  /** §24's estop bit: the hardware chain is open. */
  estop?: boolean
  /** Who holds the body (§25.2), or null when nobody does. */
  lease_held_by?: string | null
  /** The motion the controller is executing, when one is in flight. */
  executing?: string | null
}

export interface MotionVerdict {
  accepted: boolean
  refusal: MotionRefusalCode | null
  detail: string
  /**
   * The argument list the driver is handed, in the motion's **declaration order** (§88.2).
   *
   * A program-level controller's motion is a call with positional arguments and the declaration
   * order is the only order anybody agreed on. A parameter the motion declares, does not require,
   * and the request omits is **absent rather than zero** — the controller owns its defaults, and
   * substituting one here would be inventing a command. Empty on a refusal.
   */
  arguments: Array<{ name: string; value: number }>
  /**
   * Parameters whose value nothing checked, because the motion declares no bound for them (§5.2).
   *
   * Absent `min`/`max` means nobody declared one, never *"unbounded"*. A gate that refused an
   * unbounded parameter would refuse most real controllers' most useful motions; one that silently
   * passed it would be reporting a check it did not perform. This is the third option, and it is the
   * shape §7.2 already uses for a guard whose input nobody supplies.
   */
  unbounded: string[]
}

/** Whether a parameter is required. §84.1's fail-closed reading: anything but explicit `false`. */
function required(parameter: MotionParameter): boolean {
  return parameter.required !== false
}

function refuse(refusal: MotionRefusalCode, detail: string): MotionVerdict {
  return { accepted: false, refusal, detail, arguments: [], unbounded: [] }
}

/**
 * Whether this request may be handed to the driver, and with what.
 *
 * Authority before state, state before content. A request from a holder that does not hold the body
 * is refused **without the platform reading what it asked for**, because reading it would mean
 * answering a question nobody was entitled to ask.
 *
 * `busy` sits above `unknown_motion` for the reason it sits where it does in every driver: a
 * controller running a program is not in a position to be told about the request at all, and *"the
 * robot is mid-motion"* and *"you asked for something this robot cannot do"* are two things an
 * operator needs to be able to tell apart rather than a precedence puzzle.
 *
 * **`parameter_unknown` is a refusal here and a warning at plan time, and the asymmetry is the
 * point.** §84.3 raises `motion_parameter_unused` on a transport plan and does not refuse, *"because
 * the controller takes the motion"* — a policy that believes it sets a blend radius and does not is
 * something to characterise, not to block at install. At command time the same fact is a value about
 * to be handed to a controller that will either interpret it or reject it on a moving robot, and
 * §25.2's argument applies unchanged: commanding a body with the parts of a command it happened to
 * understand is worse than refusing. So the plan-time warning is exactly the install-time notice of
 * this command-time refusal, which is what a warning should be and rarely is.
 */
export function admitMotion(gate: MotionGate, request: MotionRequest): MotionVerdict {
  const holder = typeof request.holder === 'string' ? request.holder : null
  const held = typeof gate.lease_held_by === 'string' ? gate.lease_held_by : null

  if (held === null || holder === null || held !== holder) {
    return refuse('no_lease', held === null
      ? 'Nothing holds this body, so there is nobody whose motion this would be (§25.2).'
      : `This body is held by \`${held}\` and the request came from `
        + `\`${holder ?? 'nobody'}\` (§25.2).`)
  }

  if (gate.estop === true) {
    return refuse('estop', 'The emergency-stop chain is open (§24). What opened it is not something '
      + 'the body can say, so the only correct response is to command nothing.')
  }

  if (!(MOTION_READY_STATES as readonly string[]).includes(gate.state)) {
    return refuse('not_active', `§27 has this body in \`${String(gate.state)}\`, which is not a `
      + 'state in which it will run a program. Accepting the request would be queueing work behind '
      + "whatever the body is doing instead — a controlled stop, a self-test, or waiting to be "
      + 'homed.')
  }

  if (typeof gate.executing === 'string' && gate.executing !== '') {
    return refuse('busy', `The controller is executing \`${gate.executing}\`. A program-level `
      + 'controller owns the servo loop (§59.2), so it finishes or it is aborted; there is no '
      + 'second motion to interleave with the first.')
  }

  const motion = gate.motions.find(candidate => candidate.name === request.motion)
  if (motion === undefined) {
    return refuse('unknown_motion', `This body declares no motion called \`${String(request.motion)}\`. `
      + 'The name a request carries is the **body\'s** spelling, which §84.3\'s plan resolved at '
      + 'install — a policy\'s own name reaching here unresolved is the defect this refusal usually '
      + 'means.')
  }

  const deadline = request.deadline_ms
  if (deadline === null || deadline === undefined || !Number.isFinite(deadline) || deadline <= 0) {
    return refuse('deadline_undeclared', 'A motion takes seconds, during which §7.1\'s watchdog '
      + 'guard has nothing to watch: a correct policy produces no command at all. The deadline is '
      + 'what replaces it (§88.3), so a request without one is a program nothing would ever '
      + 'conclude had failed.')
  }

  const declared = new Map<string, MotionParameter>()
  for (const parameter of motion.parameters) {
    // First wins, as §84.1's reader does with a repeated name: `duplicate_parameter` is the
    // descriptor's fault and `embodiment.descriptor.valid` is where it is reported. A gate that
    // preferred the later one would command against a bound the check never looked at.
    if (!declared.has(parameter.name)) declared.set(parameter.name, parameter)
  }

  // The request's own names first: a parameter the motion has no slot for means the policy and the
  // body disagree about the motion's *shape*, which subsumes arguing about its values.
  for (const name of Object.keys(request.parameters).sort()) {
    if (declared.has(name)) continue
    return refuse('parameter_unknown', `\`${motion.name}\` does not take \`${name}\`. §84.3 warned `
      + 'about this at install — `motion_parameter_unused` — and declined to refuse a plan over it, '
      + 'because the controller does take the motion. Here it is a value about to be sent to a '
      + 'controller that has no slot for it, and §25.2 is clear that commanding with the parts of a '
      + 'command a machine happened to understand is worse than refusing.')
  }

  const args: Array<{ name: string; value: number }> = []
  const unbounded: string[] = []

  for (const parameter of declared.values()) {
    const supplied = Object.prototype.hasOwnProperty.call(request.parameters, parameter.name)
    if (!supplied) {
      if (required(parameter)) {
        return refuse('parameter_missing', `\`${motion.name}\` requires \`${parameter.name}\` and `
          + 'the request does not carry it. §84.1 reads a requirement fail-closed, so a parameter '
          + 'whose `required` the descriptor never declared is required here too.')
      }
      // Absent rather than zero. §7.2's rule, and the controller owns its own defaults.
      continue
    }

    const value = request.parameters[parameter.name]
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      return refuse('parameter_not_finite', `\`${parameter.name}\` is not a finite number. A `
        + 'controller handed one would either fault or move somewhere nobody chose.')
    }

    const min = parameter.min
    const max = parameter.max
    const hasMin = min !== undefined && Number.isFinite(min)
    const hasMax = max !== undefined && Number.isFinite(max)
    if (hasMin && value < (min as number)) {
      return refuse('parameter_out_of_bounds', `\`${parameter.name}\` is ${value} and `
        + `\`${motion.name}\` declares a minimum of ${min as number}.`)
    }
    if (hasMax && value > (max as number)) {
      return refuse('parameter_out_of_bounds', `\`${parameter.name}\` is ${value} and `
        + `\`${motion.name}\` declares a maximum of ${max as number}.`)
    }
    if (!hasMin && !hasMax) unbounded.push(parameter.name)

    args.push({ name: parameter.name, value })
  }

  return {
    accepted: true,
    refusal: null,
    detail: unbounded.length === 0
      ? `\`${motion.name}\` accepted, with every supplied parameter inside a bound this body `
        + 'declared.'
      : `\`${motion.name}\` accepted. ${unbounded.length === 1 ? 'One parameter was' : 'Some parameters were'} `
        + 'not checked against anything, because this body declares no bound for '
        + `${unbounded.join(', ')} — which §5.2 reads as *nobody declared one*, never as unbounded.`,
    arguments: args,
    unbounded,
  }
}

export const MOTION_PROGRESS = ['running', 'overdue'] as const
export type MotionProgress = (typeof MOTION_PROGRESS)[number]

export interface MotionProgressReport {
  progress: MotionProgress
  /** Milliseconds past the deadline, or the time left. Negative is remaining. */
  overdue_by_ms: number
  detail: string
}

/**
 * Whether a motion in flight has run past its deadline (§88.3).
 *
 * Separate from [`admitMotion`] because one is asked **once per request** and this is asked **every
 * cycle** until the controller answers. Folding them together would produce a function whose
 * expensive half ran at the control rate, and — worse — one that could not be read without knowing
 * which of its two jobs a caller wanted.
 *
 * Past the deadline with no completion, the driver is asked to abort, §27's fault path is entered,
 * and the black box carries the request, the verdict and the outcome. **Every request and every
 * verdict is recorded, accepted and refused alike**: §26's completeness argument is what makes an
 * episode evidence, and a program-level body whose recording held only the motions that ran could
 * not distinguish *"the skill asked for nothing"* from *"the skill was refused ten times"*.
 */
export function motionProgress(
  deadlineMs: number,
  startedAtMs: number,
  nowMs: number,
): MotionProgressReport {
  const elapsed = nowMs - startedAtMs
  const over = elapsed - deadlineMs
  // `>` rather than `>=`, as §87.4's lease is: the instant the deadline falls is still inside it,
  // and a controller that answers exactly on time has answered on time.
  if (over > 0) {
    return {
      progress: 'overdue',
      overdue_by_ms: over,
      detail: `The controller has held this motion for ${elapsed} ms against a ${deadlineMs} ms `
        + 'deadline. §7.1\'s watchdog has nothing to watch on this path (§88.3), so this is the '
        + 'bound that fires: abort the motion, enter §27\'s fault path, and record it.',
    }
  }
  return {
    progress: 'running',
    overdue_by_ms: over,
    detail: `Inside its deadline, with ${-over} ms left.`,
  }
}
