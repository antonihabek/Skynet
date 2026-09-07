/**
 * The operating state machine (§27, extended by §48.1).
 *
 * Six sections have referred to this machine and none of them declared it. §48's power-on self-test
 * describes "the state §48.1's machine should be in after this" and returns a string. §49.1's
 * warning tone sounds "on transition into `RUNNING`" and took two strings, with a comment saying so
 * and saying why. §33.4's safe-state barrier takes a boolean somebody hands it. §54.4's skill
 * lifecycle waits on it. A state machine that four callers describe in prose and nobody writes down
 * is a state machine with four definitions, and the last two sessions recorded that as a finding
 * rather than fixing it.
 *
 * This is it.
 *
 * ── The states ─────────────────────────────────────────────────────────────
 *
 * §27's five, with §48.1's two in front of them:
 *
 *     boot → POST → UNHOMED → HOLD → RUNNING …
 *              ↓ (POST failure)
 *            SAFE
 *
 * | State | Entered when | Behaviour |
 * |---|---|---|
 * | `post` | Booting; the self-test has not established that this robot may run | Nothing moves |
 * | `unhomed` | Joint positions are unknown, so every pose-dependent guard is unavailable | Only homing may move it |
 * | `hold` | No policy, or a guard `hold` | Setpoints held, brakes as configured. The safe resting state |
 * | `running` | Lease granted, plan resolved, kernel live | Normal operation |
 * | `degraded` | A guard is unavailable, or the RHAL reports `RHAL_DEGRADED` | Runs, reports, refuses packages whose grants depend on the failed guard |
 * | `stopping` | Watchdog, proximity, balance, or an operator stop | Controlled ramp per the stop category |
 * | `safe` | Stop complete, or the kernel lost the RHAL | Drives disabled. **Requires an explicit human action to leave** |
 *
 * ── It is a function of what is observed, not a table of edges ─────────────
 *
 * The design decision that shapes everything below, and it is §33.3's, applied to state rather than
 * to packages:
 *
 * > An edge-triggered supervisor acts on transitions … Miss one edge, because the process was
 * > restarting or the disk was full or the network dropped mid-message, and the system stays wrong
 * > forever. A level-triggered supervisor asks "what is true, what should be true, what is the
 * > difference" and converges from *any* starting state, including states nobody anticipated.
 *
 * §27 gives a table of **entry conditions**, not a list of edges, and that is the shape it should
 * keep. [`evaluate`] is a pure function of the conditions observed right now, in a fixed priority
 * order, and every state is reachable from every other one by the conditions changing. There is no
 * edge to miss, no transition anybody has to remember to fire, and recovery is not a special case —
 * a guard coming back is the same evaluation over a different input.
 *
 * ── The machine has exactly one bit of memory, and that bit is `SAFE` ──────
 *
 * `previous` is an argument for one reason:
 *
 * > `SAFE` is deliberately sticky. A robot that recovers itself from a safety stop is a robot that
 * > resumes moving while somebody is standing next to it working out what happened.
 *
 * Everything else is derived from conditions. Being explicit about the one exception is what keeps
 * it from quietly becoming three: a machine that remembered why it was degraded, or which policy
 * was running before the stop, would be a machine whose state could disagree with the robot's.
 *
 * ── Leaving `SAFE` re-runs the self-test, and nothing here decides that ────
 *
 * §27 says leaving `SAFE` "requires an explicit human action" and does not say where it goes. This
 * file does not invent a destination: a clearance clears the **stickiness**, and the state is then
 * whatever the conditions say it is. That has the property worth having and no design in it — an
 * operator clearing `SAFE` on a robot whose POST still fails, or whose estop is still pressed, gets
 * `SAFE` straight back, with the reason that put it there rather than the reason they cleared.
 *
 * ── The ordering is the asymmetry §33.4 states out loud ────────────────────
 *
 * > Anything that reduces what the robot may do applies immediately … because making a robot less
 * > capable is never the dangerous direction. This asymmetry appears throughout the design and it
 * > is deliberate.
 *
 * So the rules are tried least-capable-first. Where two conditions both hold, the one that stops
 * the robot wins, and the reason reported is that one — which is also what an operator needs,
 * because the condition that is holding the robot is the condition they have to clear.
 */

/**
 * §27's states, ordered least capable first.
 *
 * The order is the one the priority rules in [`evaluate`] descend through, and it is worth having
 * them agree: a reader checking whether a rule is in the right place can read this list.
 */
export const OPERATING_STATES = [
  'safe',
  'stopping',
  'post',
  'unhomed',
  'hold',
  'degraded',
  'running',
] as const

export type OperatingState = (typeof OPERATING_STATES)[number]

/**
 * What §48.2's self-test concluded, as the one input §27's machine takes from it.
 *
 * Three answers rather than §48.4's four outcomes, because the four are per **check** and this is
 * the report: every check passed, something failed, or neither — which is the distinction §48.4
 * spends its length on. `incomplete` is not a softening of `failed`; it is the state of a robot
 * whose checks could not run, and §48.4 is explicit that such a robot "does not need a human. It
 * needs the rest of the operating system."
 */
export const POST_VERDICTS = ['passed', 'failed', 'incomplete'] as const

export type PostVerdict = (typeof POST_VERDICTS)[number]

/**
 * Whether this body knows where its joints are (§48.3).
 *
 * `not_required` is a declaration, not an absence: "A body that needs no homing declares so and the
 * state is skipped." A body that has said nothing is one whose joint positions are unknown, so the
 * default is `pending` and the reader below is written to make that the answer for a document that
 * does not mention homing.
 */
export const HOMING_STATES = ['not_required', 'pending', 'complete'] as const

export type HomingState = (typeof HOMING_STATES)[number]

/**
 * What the RHAL is reporting, as far as the kernel can tell.
 *
 * `lost` is separate from `degraded` because §27 puts them in different rows and the difference is
 * the whole of `SAFE`: a degraded RHAL is a body that is still there and still answering, and a
 * lost one is a body the kernel can no longer command or observe.
 */
export const RHAL_STATES = ['present', 'degraded', 'lost'] as const

export type RhalState = (typeof RHAL_STATES)[number]

/**
 * §27's causes of a controlled stop. Five since 2026-09-07, and the fifth is a different kind.
 *
 * The list is closed on purpose. A cause added here is another thing that stops a robot, and it
 * should be added by somebody who found this constant rather than by somebody who found a boolean
 * called `stopping` and set it.
 *
 * The first four are the **kernel's own decisions** — it evaluated a guard, or a person asked, and
 * it commanded a stop. `estop` is not one of those: it is the body reporting that the hardware
 * emergency-stop chain is open, which happened whether or not any software agreed. §24 gives it one
 * bit in the state flags, so a robot can say the chain is open and cannot say what opened it.
 *
 * That is why it is not folded into `operator`. `operator` means *a person asked*, and an estop
 * chain is also opened by a light curtain, a door interlock and a safety relay noticing itself
 * fail. Reporting those as a person having asked would put a name on an event that has none, and
 * §64 makes a message that misdescribes what happened a defect rather than a wording choice.
 */
export const STOP_CAUSES = ['watchdog', 'proximity', 'balance', 'operator', 'estop'] as const

export type StopCause = (typeof STOP_CAUSES)[number]

/**
 * Whether there is a policy the robot could be running (§27's `RUNNING` row).
 *
 * `ready` is all three of "lease granted, plan resolved, kernel live" — the first two, at least;
 * the kernel is its own condition below because §27 states it separately and because a robot with a
 * ready policy and a dead kernel is the one case the document refuses outright.
 *
 * `held` is a policy that exists and is not ready: staged, refused a lease, or waiting on a plan.
 * It is distinct from `none` because the two send an operator to different places, and neither runs.
 */
export const POLICY_STATES = ['none', 'held', 'ready'] as const

export type PolicyState = (typeof POLICY_STATES)[number]

/** Whether the Safety Kernel is live. §27: "a robot with a policy and no kernel must not exist". */
export const KERNEL_STATES = ['live', 'down'] as const

export type KernelState = (typeof KERNEL_STATES)[number]

/**
 * Everything the machine looks at, and nothing it remembers.
 *
 * Every field is observed rather than inferred, which is the same discipline §33.3 imposes on the
 * reconciler's `actual`: a machine that trusted its own record of the last evaluation would be a
 * machine that disagrees with the robot, and on a robot that disagreement is discovered by a person.
 *
 * Every field is optional and every default is the least capable reading. A caller who has not
 * observed something has not established it, and a state machine that read "nobody told me about
 * the kernel" as "the kernel is live" would be the single most dangerous line in this repository.
 */
export interface Conditions {
  /** §48.2's report, rolled up. Absent means the self-test has not been run: `incomplete`. */
  post?: PostVerdict
  /** §48.3. Absent means unknown, which is `pending`. */
  homing?: HomingState
  /** §27. Absent means `down`, because nobody has said it is live. */
  kernel?: KernelState
  /** §27. Absent means `lost`, for the same reason. */
  rhal?: RhalState
  /** §27's `STOPPING` row. Absent, or `null`, means no stop is commanded. */
  stop?: StopCause | null
  /**
   * Whether the ramp has finished. §27: `SAFE` is entered when the "stop complete".
   *
   * Read only when a stop is commanded. A robot that is not stopping has not completed a stop, and
   * a caller leaving this true between stops would otherwise pin the machine in `SAFE`.
   *
   * **For `estop` it is not observable on any edition that exists**, and that is a property of the
   * evidence rather than an omission. §24 gives the hardware chain one bit, which says the chain is
   * open and says nothing about whether the body has come to rest — so a supplier that set this
   * true for a hardware stop would be claiming to have watched a robot stop moving. Absent, it
   * derives `stopping`, which can hold a robot and can never release one.
   */
  stop_settled?: boolean
  /** §27's `HOLD` row: "a guard `hold`". */
  guard_hold?: boolean
  /** §27's `DEGRADED` row: "a guard is unavailable". §45.5 is the usual source. */
  guards_unavailable?: boolean
  /** §27's `RUNNING` row: "lease granted, plan resolved". Absent means `none`. */
  policy?: PolicyState
  /**
   * The explicit human action §27 requires to leave `SAFE`.
   *
   * A string rather than a boolean because it is recorded: §13's black box takes the transition, and
   * "somebody cleared the safety stop" is worth exactly as much as the name of the somebody. An
   * empty string is not a clearance — a caller that had nobody to name should not have one.
   */
  cleared_by?: string | null
}

/**
 * Why the machine is where it is, in the vocabulary a console renders.
 *
 * One reason per rule, so the ordering below is legible from the output: a robot reporting
 * `no_policy` is a robot for which every rule above that one did not fire, and that is a stronger
 * statement than the state alone. `hold` for a guard hold and `hold` for no policy are the same
 * state and completely different situations, and an operator staring at a robot that will not move
 * needs the second fact rather than the first.
 */
export const STATE_REASONS = [
  'rhal_lost',
  'safe_sticky',
  'post_failed',
  'stop_complete',
  'stopping_watchdog',
  'stopping_proximity',
  'stopping_balance',
  'stopping_operator',
  'stopping_estop',
  'post_incomplete',
  'homing_pending',
  'kernel_down',
  'guard_hold',
  'no_policy',
  'policy_not_ready',
  'guards_unavailable',
  'rhal_degraded',
  'nominal',
] as const

export type StateReason = (typeof STATE_REASONS)[number]

/** One evaluation of the machine. */
export interface Transition {
  from: OperatingState
  to: OperatingState
  reason: StateReason
  /**
   * Whether the state changed.
   *
   * Separate from `from !== to` only in that it is named: the reconciler runs this every pass and
   * most passes change nothing, and a caller filtering black box events on a comparison it wrote
   * itself is a caller that will eventually write `!==` where it meant `===`.
   */
  changed: boolean
  /** §49.1's tone. See [`warningRequired`]. */
  warning: boolean
}

/**
 * Whether a state is one §33.4's barrier may open at.
 *
 * > A safe state: nothing is held, nothing is mid-task.
 *
 * `stopping` is not one, and it is the interesting entry: a robot mid-ramp is the state that most
 * looks like it is about to be safe and most certainly is not. Swapping a driver during the ramp
 * would change the meaning of the deceleration in flight.
 *
 * `safe` is one, and `post` and `unhomed` are, because nothing in any of them is moving or holding
 * a task. That is what makes an OS update apply on a robot that failed its self-test, which is the
 * behaviour to want: the remedy for a robot in `SAFE` is often the update.
 */
export function isSafeState(state: OperatingState): boolean {
  return state === 'post' || state === 'unhomed' || state === 'hold' || state === 'safe'
}

/**
 * Whether a policy is commanding the body in this state.
 *
 * `running` and `degraded`, and the second is the one that had been getting this wrong. §27's
 * `DEGRADED` row says the robot "**runs**, reports, and refuses any package whose grants depend on
 * the failed guard" — it is the impaired *running* state, not a third resting one. That is why the
 * degraded rule sits below the policy rules in [`evaluate`]: a robot with an unavailable guard and
 * no policy is holding, and calling that `degraded` would report a robot as running when it is not.
 */
export function commandsAutonomously(state: OperatingState): boolean {
  return state === 'running' || state === 'degraded'
}

/** Whether a state requires an explicit human action to leave (§27). Exactly one does. */
export function isSticky(state: OperatingState): boolean {
  return state === 'safe'
}

/**
 * Whether a warning tone must sound on this transition (§49.1).
 *
 * > Standards for robots operating around people expect an audible indication before autonomous
 * > motion. That makes the warning tone a **system function, not an application feature**:
 * > `skynetd` owns it, it sounds on transition into `RUNNING` and on resumption after a stop, and
 * > **a package cannot suppress it.**
 *
 * This function used to live in `package.ts` beside the reserved-token half of §49.1, took two
 * strings, and compared them to the literal `'running'`. It was under-inclusive and the comment
 * explaining why it took strings is what makes that findable: `hold → degraded` is a policy
 * beginning to command a body, and it is not a transition into `running`, so the tone did not
 * sound. A robot whose guard set is imperfect — which is every robot with a lapsed calibration,
 * §45.5's ordinary case — would have begun autonomous motion silently.
 *
 * Phrased over [`commandsAutonomously`] the two spellings cannot come apart, and the rule matches
 * the sentence it implements rather than one example from it. "Resumption after a stop" falls out:
 * `stopping` and `safe` command nothing, so leaving either for a commanding state sounds.
 */
export function warningRequired(from: OperatingState, to: OperatingState): boolean {
  return commandsAutonomously(to) && !commandsAutonomously(from)
}

/** A clearance is a named human action. An empty name is not one. */
function cleared(conditions: Conditions): boolean {
  return typeof conditions.cleared_by === 'string' && conditions.cleared_by.length > 0
}

/**
 * The reason a stop is in progress, by cause.
 *
 * A `switch` rather than a template literal, so an added [`StopCause`] is a compile error in the
 * Rust edition and a case the corpus rejects here, instead of a reason string nothing declares.
 */
function stoppingReason(cause: StopCause): StateReason {
  switch (cause) {
    case 'watchdog': return 'stopping_watchdog'
    case 'proximity': return 'stopping_proximity'
    case 'balance': return 'stopping_balance'
    case 'operator': return 'stopping_operator'
    case 'estop': return 'stopping_estop'
  }
}

/** The state and reason the conditions imply, before stickiness. */
function derive(conditions: Conditions): { state: OperatingState, reason: StateReason } {
  const rhal = conditions.rhal ?? 'lost'
  const stop = conditions.stop ?? null

  // 1. The kernel lost the body. §27 puts this in the `SAFE` row beside a completed stop, and it
  //    is first because it is the condition under which nothing else can be known: a robot whose
  //    RHAL is gone has no observed joint state, no estop reading and no guard that can evaluate.
  if (rhal === 'lost') return { state: 'safe', reason: 'rhal_lost' }

  // 2. A failed self-test. §48.2: `SAFE`, not `DEGRADED`, "because a robot that failed its own
  //    self-test should not be talked into running by a reconcile loop".
  if (conditions.post === 'failed') return { state: 'safe', reason: 'post_failed' }

  // 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever
  //    the robot is doing, including booting.
  if (stop !== null) {
    return conditions.stop_settled === true
      ? { state: 'safe', reason: 'stop_complete' }
      : { state: 'stopping', reason: stoppingReason(stop) }
  }

  // 4. The self-test has not established that this robot may run. Not a fault: §48.4's
  //    `post_incomplete`, which is a robot waiting on the rest of the operating system.
  if ((conditions.post ?? 'incomplete') !== 'passed') {
    return { state: 'post', reason: 'post_incomplete' }
  }

  // 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every
  //    pose-dependent guard is unavailable — so a policy must not run, and homing is the only
  //    motion permitted (§48.3).
  if ((conditions.homing ?? 'pending') === 'pending') {
    return { state: 'unhomed', reason: 'homing_pending' }
  }

  // 6. §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start
  //    if the kernel is not live". Refusing to start is `HOLD` — the safe resting state — rather
  //    than `SAFE`: nothing has gone wrong with the robot, and a sticky state needing a human
  //    would be the wrong answer for a kernel that is merely still starting.
  if ((conditions.kernel ?? 'down') !== 'live') return { state: 'hold', reason: 'kernel_down' }

  // 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like
  //    to run, which is the whole point of a guard.
  if (conditions.guard_hold === true) return { state: 'hold', reason: 'guard_hold' }

  // 8. §27's `HOLD` row, second half: "no policy". Split in two, because "there is nothing to run"
  //    and "there is something and it has not got a lease" are different sentences and the second
  //    is the one with a remedy — §25.2's arbitration is where an operator goes to find out who
  //    else is holding the arm.
  const policy = conditions.policy ?? 'none'
  if (policy === 'none') return { state: 'hold', reason: 'no_policy' }
  if (policy !== 'ready') return { state: 'hold', reason: 'policy_not_ready' }

  // 9. §27's `DEGRADED` row, and it is below the policy rules deliberately — see
  //    [`commandsAutonomously`]. Reaching here means a policy is ready to command, so `degraded` is
  //    the impaired running state rather than a resting one.
  if (conditions.guards_unavailable === true) {
    return { state: 'degraded', reason: 'guards_unavailable' }
  }
  if (rhal === 'degraded') return { state: 'degraded', reason: 'rhal_degraded' }

  return { state: 'running', reason: 'nominal' }
}

/**
 * The state this robot is in, given where it was and what is true now.
 *
 * Pure, clockless and total. Every combination of conditions produces a state, including
 * combinations nobody anticipated, which is the property §33.3 asks of anything that converges.
 */
export function evaluate(previous: OperatingState, conditions: Conditions = {}): Transition {
  const derived = derive(conditions)

  // §27's stickiness, applied after the derivation rather than instead of it. A robot in `SAFE`
  // whose conditions would put it back in `SAFE` reports the *condition's* reason rather than
  // `safe_sticky`, because "the estop is still pressed" is what an operator needs and "you have
  // not cleared it yet" is not.
  const stuck = previous === 'safe' && !cleared(conditions) && derived.state !== 'safe'
  const to = stuck ? 'safe' : derived.state
  const reason = stuck ? 'safe_sticky' : derived.reason

  return {
    from: previous,
    to,
    reason,
    changed: to !== previous,
    warning: warningRequired(previous, to),
  }
}
