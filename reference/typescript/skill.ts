/**
 * Skills — preconditions, success, timeout and composition (§54).
 *
 * A skill is not a policy, and the distinction is the reason this file exists:
 *
 * | | **Policy** | **Skill** |
 * |---|---|---|
 * | Shape | Observations → actions, continuously | Invoked with arguments, terminates |
 * | Duration | As long as it holds the lease | Bounded, declared |
 * | Outcome | None; it just runs | Succeeded, failed or aborted |
 * | Guards | Envelope applies | Envelope applies, **plus preconditions** |
 *
 * Everything here is pure, **including the lifecycle**. `idle → preconditions → acquiring → running
 * → verifying → terminal` was recorded for two sessions as waiting on capability leases and on
 * §27's state machine. Half of that was already wrong when it was written — `skynetd::lease` has
 * held §25.2's table since the fifteenth session — and the other half stopped being true when
 * `opstate.ts` was declared. So [`advance`] is here, and it is a function of a phase and the
 * evidence gathered so far rather than a loop.
 *
 * What is still missing is the loop itself: something that calls this every cycle, runs the skill's
 * policy, and writes the black box events. The decisions it will make are frozen and testable, so
 * when something does loop it is not also inventing the rules.
 *
 * ── The asymmetry, which is the substantive decision in this file ───────────
 *
 * A precondition and a success criterion are the same shape — a sentence and a predicate — and the
 * OS treats *not knowing* about them in opposite ways:
 *
 * * **A precondition nobody can evaluate refuses the skill.** §54.2: "Not 'assume true', which is
 *   what free-text preconditions do by construction."
 * * **A criterion nobody can evaluate makes the outcome `unlabeled`, not `failure`.** §54.3:
 *   "Honest ignorance beats an invented pass."
 *
 * That is not an inconsistency. Before the skill runs, ignorance is a reason not to act, and acting
 * anyway is how a robot moves into a state nobody checked. After it runs, ignorance is a reason not
 * to *claim* — and calling an unmeasured run a failure would poison a dataset exactly as thoroughly
 * as calling it a success. One refuses to act on what it does not know; the other refuses to assert
 * it.
 *
 * ── What this costs the skills that exist today ────────────────────────────
 *
 * `SkillManifest.preconditions` is `string[]` — prose — and prose has no predicate, so under
 * §54.2's rule every published skill refuses. That is stated here rather than softened, because it
 * is the whole point: a skill whose safety preconditions are sentences has not been made safe, it
 * has been described. [`evaluatePreconditions`] reports each unenforceable statement individually
 * rather than a single verdict, so the report is a publisher's to-do list in the same way
 * `evaluateReadiness`'s blockers are a vendor's.
 *
 * What the *runtime* does about a guest-tier robot running a prose skill is a tier policy this file
 * deliberately does not decide. It reports; §33 decides.
 */

import { evaluatePredicateOutcome, type Outcome, type UnevaluableReason } from './predicate.ts'
import type { Predicate } from './types.ts'
// §54.4's `running` row: "a skill is not an exemption from the kernel". The lifecycle reads
// §27's machine rather than keeping a second opinion about whether the robot is commanding.
import type { OperatingState } from './opstate.ts'

/**
 * One precondition or success criterion.
 *
 * `statement` is what a human reads and is never dropped: the predicate is what the OS enforces,
 * and an operator staring at a refused skill needs the sentence its author wrote, not
 * `robot.calibration.torque.valid`.
 */
export interface Clause {
  statement: string
  /** What the OS evaluates. Absent means the OS cannot enforce this clause at all. */
  predicate?: Predicate | null
  /** §54.2: re-checked every cycle, not only at entry. Meaningless on a success criterion. */
  hold?: boolean
}

/** Where a clause came from. §54.2: the operator may add preconditions; the skill may not remove them. */
export type ClauseOrigin = 'skill' | 'operator'

/** Both origins, publisher first — which is also the order clauses are evaluated in. */
export const CLAUSE_ORIGINS: ClauseOrigin[] = ['skill', 'operator']

export interface ResolvedClause extends Clause {
  origin: ClauseOrigin
}

/**
 * What happened to one clause.
 *
 * `unenforceable` is the fourth value the predicate engine does not have, and it belongs here
 * rather than there: the engine is asked about a predicate, and this is the case where there is no
 * predicate to ask about.
 */
export type ClauseOutcome = Outcome | 'unenforceable'

export const CLAUSE_OUTCOMES: ClauseOutcome[] = [
  'satisfied', 'unsatisfied', 'unevaluable', 'unenforceable',
]

export interface ClauseResult {
  index: number
  statement: string
  origin: ClauseOrigin
  hold: boolean
  outcome: ClauseOutcome
  /** Why, when the predicate could not be decided. Null otherwise. */
  reason: UnevaluableReason | null
  /** The path that could not be resolved, when there is one. */
  path: string | null
  /** The operator that could not decide, when there was one. */
  op: string | null
}

/** §54.2's two moments: before the skill starts, and every cycle while it runs. */
export type Phase = 'entry' | 'hold'

export const PHASES: Phase[] = ['entry', 'hold']

export interface PreconditionReport {
  phase: Phase
  /** May the skill start, or continue? Only every evaluated clause being satisfied permits it. */
  permitted: boolean
  /** The first clause that did not hold, which is what a refusal names. Null when permitted. */
  refused: ClauseResult | null
  /** Every clause this phase evaluated, in order. */
  results: ClauseResult[]
  /** How many of them declare no predicate — the publisher's to-do list, counted. */
  unenforceable: number
}

/**
 * Read a manifest's `preconditions` or `success_criteria`, in either spelling.
 *
 * A bare string is prose with no predicate, which is what every skill published before §54.2
 * carries and what this accepts so that reading one is not an error. It is not the same as
 * *absent*: a skill that declares no preconditions has nothing to check, and a skill that declares
 * three sentences has three things the OS cannot check. Those are different reports.
 */
export function readClauses(input: unknown): Clause[] {
  if (!Array.isArray(input)) return []
  const clauses: Clause[] = []
  for (const entry of input) {
    if (typeof entry === 'string') {
      clauses.push({ statement: entry })
      continue
    }
    if (!entry || typeof entry !== 'object') continue
    const record = entry as Record<string, unknown>
    const statement = typeof record.statement === 'string' ? record.statement : ''
    const predicate = record.predicate && typeof record.predicate === 'object'
      ? record.predicate as Predicate
      : null
    clauses.push({ statement, predicate, hold: record.hold === true })
  }
  return clauses
}

/**
 * The skill's clauses, then the site's. §54.2's narrowing-only rule, made structural.
 *
 * > The operator may add preconditions; the skill may not remove them. Same narrowing-only rule as
 * > profiles (§38.2). A site can require "door closed" on a skill whose publisher never imagined
 * > that cell.
 *
 * There is no argument by which the operator's list can shorten the result, because the operator's
 * list is only ever appended — a removal would need a mechanism, and not building one is the
 * enforcement. The publisher's clauses come first so that a refusal names the skill's own
 * requirement before the site's where both fail, which is the order that tells an operator whether
 * to call the publisher or walk to the cell.
 */
export function resolveClauses(published: Clause[], operator: Clause[] = []): ResolvedClause[] {
  return [
    ...published.map(clause => ({ ...clause, origin: 'skill' as const })),
    ...operator.map(clause => ({ ...clause, origin: 'operator' as const })),
  ]
}

function judgeClause(clause: ResolvedClause, index: number, context: unknown): ClauseResult {
  const base = {
    index,
    statement: clause.statement,
    origin: clause.origin,
    hold: clause.hold === true,
  }
  if (!clause.predicate) {
    return { ...base, outcome: 'unenforceable', reason: null, path: null, op: null }
  }
  const outcome = evaluatePredicateOutcome(clause.predicate, context)
  return {
    ...base,
    outcome: outcome.outcome,
    reason: outcome.reason,
    path: outcome.path,
    op: outcome.op,
  }
}

/**
 * §54.2 — may this skill start, or continue?
 *
 * `entry` evaluates every clause. `hold` evaluates only those the manifest marked, because that is
 * what runs every cycle:
 *
 * > Preconditions are re-checked, not just entry-checked. A subset marked `hold` in the manifest is
 * > evaluated every cycle; battery dropping below reserve mid-skill should stop the skill, not be
 * > discovered at the end.
 *
 * Every clause is evaluated even after one has failed, because an operator fixing four problems one
 * attempt at a time is the failure `admitPackage` already refuses to cause. `refused` names the
 * first, which is the one to put on a screen.
 */
export function evaluatePreconditions(
  clauses: ResolvedClause[], context: unknown, phase: Phase = 'entry',
): PreconditionReport {
  const results: ClauseResult[] = []
  clauses.forEach((clause, index) => {
    if (phase === 'hold' && clause.hold !== true) return
    results.push(judgeClause(clause, index, context))
  })

  const refused = results.find(result => result.outcome !== 'satisfied') ?? null
  return {
    phase,
    permitted: refused === null,
    refused,
    results,
    unenforceable: results.filter(result => result.outcome === 'unenforceable').length,
  }
}

/** §54.3/§54.4's terminal vocabulary, which is the Hub's `EpisodeOutcome` and not a second one. */
export type SkillOutcome = 'success' | 'failure' | 'aborted' | 'unlabeled'

export const SKILL_OUTCOMES: SkillOutcome[] = ['success', 'failure', 'aborted', 'unlabeled']

export interface SuccessJudgement {
  outcome: 'success' | 'failure' | 'unlabeled'
  /** The criterion that decided it: the first false one, or the first unreadable one. */
  deciding: ClauseResult | null
  results: ClauseResult[]
}

/**
 * §54.3 — did this skill succeed? Judged by the OS, over post-state.
 *
 * > A skill that judges its own success is a skill that always succeeds — and on a platform where
 * > success rates feed a marketplace, self-reported outcomes are worth nothing.
 *
 * Three answers, and the order they are decided in is the content:
 *
 * 1. **Any criterion that is definitely false is a failure**, even if another could not be read.
 *    Evidence of failure beats absence of evidence.
 * 2. **Otherwise, anything unread is `unlabeled`** — the vocabulary the episode schema already
 *    carries. Many criteria genuinely are not machine-evaluable ("the cup is upright" needs
 *    perception), and inventing a pass for those is how a leaderboard becomes fiction.
 * 3. **Only when every criterion held is it a success.**
 *
 * A skill that declares no criteria at all is `unlabeled`, not `success`. Nothing was measured, and
 * a skill that scores itself by declaring nothing is the same gaming §54.3 exists to stop.
 */
export function judgeSuccess(clauses: ResolvedClause[], postState: unknown): SuccessJudgement {
  const results = clauses.map((clause, index) => judgeClause(clause, index, postState))

  const failed = results.find(result => result.outcome === 'unsatisfied')
  if (failed) return { outcome: 'failure', deciding: failed, results }

  const unread = results.find(result => result.outcome !== 'satisfied')
  if (unread) return { outcome: 'unlabeled', deciding: unread, results }

  if (results.length === 0) return { outcome: 'unlabeled', deciding: null, results }
  return { outcome: 'success', deciding: null, results }
}

// ── §54.4 Timeout ────────────────────────────────────────────────────────────

/**
 * How far past its own estimate a skill may run before it is stopped.
 *
 * §54.4 says "times a declared overrun factor" and does not say where it is declared, which makes
 * it an operator setting with a default rather than a manifest field — a publisher choosing how
 * long past their own estimate they may run is a publisher with no timeout.
 */
export const DEFAULT_OVERRUN_FACTOR = 2

export interface Deadline {
  /** Milliseconds from the start of the skill, or null when there is nothing to enforce. */
  deadline_ms: number | null
  /** Whether the OS can enforce a timeout on this skill at all. */
  enforceable: boolean
  factor: number
}

/**
 * §54.4 — when a running skill is stopped and recorded `aborted`.
 *
 * > Timeout is enforced by the OS. `expected_duration_seconds` from the manifest, times a declared
 * > overrun factor, and past it the skill is stopped under a controlled stop and recorded as
 * > `aborted` — matching the Hub's `EpisodeOutcome` vocabulary, so an aborted skill becomes a
 * > labelled training example rather than a gap.
 *
 * A skill that declares no duration produces `enforceable: false` rather than a guessed ceiling.
 * **This function does not decide what the runtime does about that**, and the specification does not
 * either: refusing an unbounded skill and permitting one are both defensible, and inventing the
 * answer inside a pure function is how a policy nobody agreed to ends up load-bearing. It is
 * reported so that the decision has somewhere to be made.
 */
export function skillDeadline(
  expectedDurationSeconds: number | null | undefined,
  overrunFactor: number = DEFAULT_OVERRUN_FACTOR,
): Deadline {
  const factor = typeof overrunFactor === 'number' && Number.isFinite(overrunFactor) && overrunFactor >= 1
    ? overrunFactor
    : DEFAULT_OVERRUN_FACTOR
  const declared = typeof expectedDurationSeconds === 'number'
    && Number.isFinite(expectedDurationSeconds)
    && expectedDurationSeconds > 0
  if (!declared) return { deadline_ms: null, enforceable: false, factor }
  return { deadline_ms: Math.round(expectedDurationSeconds * 1000 * factor), enforceable: true, factor }
}

// ── §54.5 Composition ────────────────────────────────────────────────────────

/** One declared skill-to-skill call. */
export interface SkillCall {
  from: string
  to: string
}

/**
 * Read declared calls, skipping anything that is not one.
 *
 * A reader rather than a cast, and it exists here so that both editions discard the same malformed
 * edges at the same boundary: a typed `SkillCall[]` in Rust cannot hold `{ from: 5 }`, so if this
 * filtering lived inside [`validateComposition`] the two editions would be filtering in different
 * places and a corpus case for a malformed edge would be testing two different functions.
 */
export function readCalls(input: unknown): SkillCall[] {
  if (!Array.isArray(input)) return []
  const calls: SkillCall[] = []
  for (const entry of input) {
    if (!entry || typeof entry !== 'object') continue
    const record = entry as Record<string, unknown>
    if (typeof record.from !== 'string' || typeof record.to !== 'string') continue
    calls.push({ from: record.from, to: record.to })
  }
  return calls
}

/**
 * §54.5's default. A skill four deep is a skill whose author can still describe what it does.
 *
 * The root counts as depth 1, so a limit of 4 permits three levels of nesting beneath it.
 */
export const MAX_SKILL_DEPTH = 4

export const COMPOSITION_FAULTS = ['cycle', 'too_deep'] as const
export type CompositionFaultCode = (typeof COMPOSITION_FAULTS)[number]

export interface CompositionFault {
  code: CompositionFaultCode
  /** The chain that reached the fault, root first. The cycle's repeated element is its last entry. */
  path: string[]
  detail: string
}

/**
 * §54.5 — is this call graph installable?
 *
 * > Depth-limited (default 4) and acyclic — a cycle is refused at install by walking the declared
 * > graph, not discovered at runtime.
 *
 * Refusing at install is the whole point. A cycle discovered at runtime is discovered by a robot
 * that has already taken a lease and started moving, and "a composition system without limits
 * becomes a way to exhaust a robot" is the mildest way to put what happens next.
 *
 * The walk is depth-first in declaration order, so the chain a fault names is reproducible: an
 * edition that iterated its edges differently would refuse the same graphs and blame different
 * skills, which is the half an operator reads.
 */
export function validateComposition(
  root: string, calls: SkillCall[], maxDepth: number = MAX_SKILL_DEPTH,
): CompositionFault | null {
  const limit = typeof maxDepth === 'number' && Number.isFinite(maxDepth) && maxDepth >= 1
    ? Math.floor(maxDepth)
    : MAX_SKILL_DEPTH

  // Declaration order is preserved so a fault's path is the same in every edition.
  const edges = new Map<string, string[]>()
  for (const call of calls) {
    if (!call || typeof call.from !== 'string' || typeof call.to !== 'string') continue
    const existing = edges.get(call.from)
    if (existing) existing.push(call.to)
    else edges.set(call.from, [call.to])
  }

  let fault: CompositionFault | null = null

  const walk = (node: string, path: string[]): void => {
    if (fault) return
    if (path.length > limit) {
      fault = {
        code: 'too_deep',
        path: [...path],
        detail: `This call chain is ${path.length} skills deep; the limit is ${limit}.`,
      }
      return
    }
    for (const next of edges.get(node) ?? []) {
      if (fault) return
      if (path.includes(next)) {
        fault = {
          code: 'cycle',
          path: [...path, next],
          detail: `${next} is already in this call chain, so it would call itself.`,
        }
        return
      }
      walk(next, [...path, next])
    }
  }

  walk(root, [root])
  return fault
}
// ── §54.4 The lifecycle ──────────────────────────────────────────────────────
//
//     idle → preconditions → acquiring → running → verifying → succeeded | failed | aborted
//               ↓ refused        ↓ denied     ↓ timeout / guard trip / precondition lost
//
// | Stage | What happens |
// |---|---|
// | `preconditions` | Predicates evaluated. A failure names the predicate and refuses. |
// | `acquiring` | Capability leases taken for `required_capabilities` (§25.2). Denied if held. |
// | `running` | The skill's policy runs under the transport plan and the resolved clamps, exactly like any other policy — a skill is not an exemption from the kernel |
// | `verifying` | `success_criteria` evaluated over post-state |
// | terminal | Leases released, outcome recorded, episode emitted |
//
// ── A phase advances on evidence, and absence of evidence is not advancement ──
//
// The rule that shapes every field below. `entry_permitted` is `null` until §54.2 has actually been
// evaluated, `outcome` is `null` until §54.3 has judged, and both cases hold the lifecycle where it
// is rather than moving it on. That is the same discipline as everywhere else in this repository —
// a check that could not run has not passed — applied to a state machine rather than to a report.
//
// It also makes the machine safe to drive from a loop that runs faster than the work: calling
// `advance` every cycle while the skill's own code is still running returns `running → running`,
// and nothing has to remember that it already asked.
//
// ── The terminal vocabulary is the Hub's, and a refusal has to fit in it ─────
//
// §54.4's diagram ends in `succeeded | failed | aborted` and its next paragraph says the outcome
// matches `EpisodeOutcome`, which is `success | failure | aborted | unlabeled`. The contract uses
// the episode spelling, because that is the one that reaches a dataset — recorded as a finding by
// the seventeenth session and unchanged.
//
// So a skill refused at `preconditions`, and one denied a lease, are terminal with `aborted`, and
// the *reason* is what tells them apart. `failure` would be a lie a marketplace consumes: a skill
// that was never allowed to start did not fail at its task, and a success rate computed over
// refusals would punish publishers for operators' cell configurations.
//
// ── What aborts a running skill, and in which order ─────────────────────────
//
// §54.4 names three arrows — timeout, guard trip, precondition lost — and this adds one more from
// §54.4's own `running` row: *"a skill is not an exemption from the kernel"*. A skill whose robot
// has stopped commanding is over, whatever stopped it.
//
// The order is **causes before effects**. A timeout makes the OS stop the skill under a controlled
// stop, and a guard trip stops the robot; both therefore *produce* a robot that is no longer
// commanding. Reporting the robot state first would hide the cause behind its own consequence, so
// the specific reasons are tried first and `robot_stopped` is the fallback that catches whatever
// this machine could not name.
//
// `hold` is deliberately **not** an aborting robot state, and it is the one exclusion worth
// writing down: it is where §27's machine sits between a lease being granted and a policy actually
// commanding, so aborting on it would kill every skill in the moment it started. A guard asking for
// a hold reaches this machine as `guard_tripped`, which is the input that means it.

export const SKILL_PHASES = [
  'idle',
  'preconditions',
  'acquiring',
  'running',
  'verifying',
  'terminal',
] as const

export type SkillPhase = (typeof SKILL_PHASES)[number]

/** §25.2's arbitration, as the one fact the lifecycle needs from it. */
export const LEASE_STATES = ['pending', 'held', 'denied'] as const

export type LeaseState = (typeof LEASE_STATES)[number]

export const LIFECYCLE_REASONS = [
  'not_invoked',
  'invoked',
  'preconditions_pending',
  'preconditions_refused',
  'preconditions_met',
  'leases_held_by_root',
  'lease_pending',
  'lease_denied',
  'leases_acquired',
  'timeout',
  'hold_lost',
  'guard_tripped',
  'robot_stopped',
  'running',
  'verifying',
  'verdict_pending',
  'judged',
  'terminal',
] as const

export type LifecycleReason = (typeof LIFECYCLE_REASONS)[number]

/**
 * Everything one invocation has established so far.
 *
 * Every field is evidence somebody gathered, and every default is "not gathered". A `Progress` with
 * nothing set is an invocation nobody has made, which is `idle` — the only starting value that is
 * not a claim.
 */
export interface Progress {
  /** Somebody called `execute({ skill, arguments, timeout_s })` (§54.1). */
  invoked?: boolean
  /**
   * Whether this is the root invocation (§54.5).
   *
   * A nested call does **not** acquire: *"leases are held by the root invocation, not re-acquired
   * per call. Nested acquisition is how you get a skill deadlocking against its own child."* So a
   * non-root invocation skips `acquiring` entirely, which is that sentence made mechanical rather
   * than left as advice to whoever writes the runtime.
   */
  root?: boolean
  /** §54.2's entry evaluation. `null` until it has been run — which is not a pass. */
  entry_permitted?: boolean | null
  /** The clause a refusal names, from `PreconditionReport.refused`. */
  refused_clause?: string
  /** §25.2. */
  leases?: LeaseState
  /** The token that was refused, and who holds it — §25.2 refuses rather than interleaves. */
  lease_token?: string
  lease_holder?: string
  /** Milliseconds since the skill entered `running`. `null` while it has not. */
  elapsed_ms?: number | null
  /** §54.4's deadline, from `skillDeadline`. */
  deadline?: Deadline
  /** A `hold` precondition falsified mid-run (§54.2's second rule). */
  hold_lost?: boolean
  /** A guard tripped, which includes a guard asking for a hold. */
  guard_tripped?: boolean
  /** Where §27's machine says the robot is. */
  robot?: OperatingState
  /** The skill's own code returned. */
  finished?: boolean
  /** §54.3's judgement. `null` until it has been made — which is not a success. */
  outcome?: SkillOutcome | null
}

export interface SkillTransition {
  from: SkillPhase
  to: SkillPhase
  reason: LifecycleReason
  /** The episode outcome, present only on entering `terminal`. */
  outcome: SkillOutcome | null
  /** What a console shows beside the reason: the clause, the token, the state. */
  detail: string
  /**
   * §54.4's terminal row: *"Leases released, outcome recorded, episode emitted."*
   *
   * True only on the transition **into** `terminal`, never on re-evaluating a skill that is already
   * there. A release emitted every cycle after a skill ended would be a release racing the next
   * invocation's acquisition, which is the one way §25.2's arbitration can be lost by the thing
   * that implements it.
   *
   * And it is true on **every** path into `terminal`, including the refusals — a skill refused
   * after its leases were taken must not keep them, and the path that does that is the one where a
   * lease leaks and the next invocation is refused the arm with nothing on any screen to say why.
   */
  release_leases: boolean
  changed: boolean
}

/** Whether a robot in this state has stopped a skill that was running. See the note above. */
function robotStoppedCommanding(state: OperatingState): boolean {
  return state === 'stopping' || state === 'safe' || state === 'post' || state === 'unhomed'
}

/** Whether the deadline has passed. An unenforceable deadline never has. */
function overran(progress: Progress): boolean {
  const deadline = progress.deadline
  if (!deadline || !deadline.enforceable || deadline.deadline_ms === null) return false
  const elapsed = progress.elapsed_ms
  if (typeof elapsed !== 'number' || !Number.isFinite(elapsed)) return false
  return elapsed >= deadline.deadline_ms
}

/**
 * Why a running skill is over, or `null` if it is not.
 *
 * Causes before effects — see the note above. The order is the content of this function.
 */
function abortReason(progress: Progress): { reason: LifecycleReason, detail: string } | null {
  if (overran(progress)) {
    return {
      reason: 'timeout',
      detail: `past ${progress.deadline?.deadline_ms} ms, which is the declared estimate times `
        + `${progress.deadline?.factor}`,
    }
  }
  if (progress.hold_lost === true) {
    return {
      reason: 'hold_lost',
      detail: 'a precondition marked `hold` stopped being true, and §54.2 re-checks those every '
        + 'cycle rather than only at entry',
    }
  }
  if (progress.guard_tripped === true) {
    return { reason: 'guard_tripped', detail: 'a guard tripped, and a skill is not an exemption' }
  }
  const robot = progress.robot ?? 'post'
  if (robotStoppedCommanding(robot)) {
    return {
      reason: 'robot_stopped',
      detail: `§27 says the robot is \`${robot}\`, which is not a state a policy commands in`,
    }
  }
  return null
}

/**
 * §54.4's lifecycle, advanced by one evaluation.
 *
 * Pure, clockless and total: `elapsed_ms` is supplied for the same reason the kernel takes `now_ms`
 * as a parameter (§35.2), which is what lets a timeout be tested without waiting for one.
 */
export function advance(phase: SkillPhase, progress: Progress = {}): SkillTransition {
  const done = (
    to: SkillPhase,
    reason: LifecycleReason,
    outcome: SkillOutcome | null,
    detail: string,
  ): SkillTransition => ({
    from: phase,
    to,
    reason,
    outcome,
    detail,
    // Only on the way in. A skill already terminal releases nothing, because it has nothing.
    release_leases: to === 'terminal' && phase !== 'terminal',
    changed: to !== phase,
  })

  switch (phase) {
    case 'idle':
      return progress.invoked === true
        ? done('preconditions', 'invoked', null, 'an invocation arrived')
        : done('idle', 'not_invoked', null, 'nothing has asked for this skill')

    case 'preconditions': {
      // Absent is not permitted. A runtime that had not evaluated §54.2 yet must not be able to
      // start a skill by failing to ask — which is the shape of every bug this repository is
      // arranged around.
      if (progress.entry_permitted !== true && progress.entry_permitted !== false) {
        return done('preconditions', 'preconditions_pending', null,
          'the entry predicates have not been evaluated, and an unevaluated precondition is not a '
          + 'satisfied one')
      }
      if (progress.entry_permitted === false) {
        return done('terminal', 'preconditions_refused', 'aborted',
          progress.refused_clause
            ? `refused by: ${progress.refused_clause}`
            : 'refused by a precondition')
      }
      // §54.5. A nested invocation does not acquire; the root holds the leases for the whole tree.
      return progress.root === false
        ? done('running', 'leases_held_by_root', null,
          'a nested call runs under the root invocation\'s leases (§54.5)')
        : done('acquiring', 'preconditions_met', null, 'every evaluated precondition held')
    }

    case 'acquiring': {
      const leases = progress.leases ?? 'pending'
      if (leases === 'denied') {
        return done('terminal', 'lease_denied', 'aborted',
          progress.lease_token
            ? `\`${progress.lease_token}\` is held by ${progress.lease_holder || 'another package'}`
            : 'a required capability is held by another package')
      }
      if (leases !== 'held') {
        return done('acquiring', 'lease_pending', null,
          'the leases for `required_capabilities` have not been taken')
      }
      return done('running', 'leases_acquired', null, 'every required capability is leased')
    }

    case 'running': {
      const aborted = abortReason(progress)
      if (aborted) return done('terminal', aborted.reason, 'aborted', aborted.detail)
      return progress.finished === true
        ? done('verifying', 'verifying', null, 'the skill returned; §54.3 has not judged it yet')
        : done('running', 'running', null, 'the skill is running')
    }

    case 'verifying': {
      // A skill can still be stopped while its post-state is being judged, and the abort outranks
      // the verdict: a skill whose robot fell over during verification did not succeed at anything.
      const aborted = abortReason(progress)
      if (aborted) return done('terminal', aborted.reason, 'aborted', aborted.detail)
      const outcome = progress.outcome
      if (outcome === undefined || outcome === null) {
        return done('verifying', 'verdict_pending', null,
          'the success criteria have not been judged, and §54.3 will not let the skill judge itself')
      }
      return done('terminal', 'judged', outcome, `§54.3 judged this ${outcome}`)
    }

    case 'terminal':
      return done('terminal', 'terminal', progress.outcome ?? null, 'this invocation is over')
  }
}

/**
 * §54.5's third limit: *"The root's timeout bounds the whole tree. A child cannot extend its
 * parent's budget."*
 *
 * The child's own deadline, or what is left of the root's, whichever is sooner. Two properties fall
 * out and both are the point:
 *
 * * a child that declares **no** duration is still bounded, by its parent, so an unbounded skill
 *   nested under a bounded one does not make the tree unbounded;
 * * a root with nothing left bounds its child at zero rather than at a negative number, so the
 *   child is stopped rather than being handed a deadline already in the past that some other
 *   arithmetic could read as "no deadline".
 */
export function boundedDeadline(child: Deadline, rootRemainingMs: number | null): Deadline {
  if (rootRemainingMs === null || !Number.isFinite(rootRemainingMs)) return child
  const remaining = Math.max(0, rootRemainingMs)
  if (!child.enforceable || child.deadline_ms === null) {
    return { deadline_ms: remaining, enforceable: true, factor: child.factor }
  }
  return {
    deadline_ms: Math.min(child.deadline_ms, remaining),
    enforceable: true,
    factor: child.factor,
  }
}
