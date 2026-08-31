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
 * Everything here is pure. The lifecycle that calls it — `idle → preconditions → acquiring →
 * running → verifying → terminal` — needs capability leases and §27's state machine, neither of
 * which exists yet. These are the decisions that lifecycle will make, written down and frozen now
 * so that when something does loop, it is not also inventing the rules.
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
