/**
 * Declarative predicate evaluation.
 *
 * Compatibility rules are data, not code, so a third party can register a robot, an adapter or
 * a hardware constraint without shipping a frontend change. This module is deliberately pure
 * and total: it never throws on a malformed rule, and a rule it cannot decide does not pass.
 *
 * ── The two answers, and why there are two ──────────────────────────────────
 *
 * [`evaluatePredicate`] returns a **boolean**, which is what a marketplace compatibility check
 * wants: a rule either fires or it does not, and one bad rule row must not take down the check for
 * every user.
 *
 * [`evaluatePredicateOutcome`] returns **three** values — satisfied, unsatisfied, unevaluable —
 * because §54.2 asks a question the boolean cannot answer:
 *
 * > A precondition that cannot be evaluated refuses the skill, **naming the path that was
 * > unresolvable**. Not "assume true", which is what free-text preconditions do by construction.
 *
 * `false` refuses, which is the right direction, but it collapses *"the world is not ready"* and
 * *"this manifest names something this robot does not expose"* into one answer. Those are different
 * facts with different remedies: the first is a battery that needs charging, the second is a skill
 * that will never run on this robot and whose publisher needs telling.
 *
 * **The boolean is derived from the outcome**, and that is the whole design. Two functions
 * computing the same thing by two routes is the drift this repository exists to prevent, so there
 * is one evaluator and `evaluatePredicate` is `satisfied(...)` of it. The refinement cannot become
 * a second opinion, because there is nothing for it to disagree with.
 *
 * ── What deriving it changed ────────────────────────────────────────────────
 *
 * Exactly one class of answer, and every instance of it moved in the same direction: **where the
 * engine used to turn ignorance into a pass, it now refuses.**
 *
 * | Input | Was | Is |
 * |---|---|---|
 * | `not` over a predicate whose path does not resolve | `true` | `unevaluable` → `false` |
 * | `not` with no operand at all | `true` | `unevaluable` → `false` |
 * | `neq` against a path that does not resolve | `true` | `unevaluable` → `false` |
 * | `eq` where neither side resolves (`undefined === undefined`), `{"op":"eq"}` included | `true` | `unevaluable` → `false` |
 * | `includes_all` / `includes_any` whose `__ref` does not resolve — vacuously true over nothing | `true` | `unevaluable` → `false` |
 *
 * Nothing moved the other way — no input that was `false` is now `true` — and that is checkable
 * rather than asserted: `corpus/predicate/evaluate.json` freezes both answers for every case,
 * including all five rows above.
 *
 * The old behaviour was not a deliberate choice. It was JavaScript's `undefined` arriving at `!`,
 * `!==` and `[].every(…)`, and it meant a compatibility rule reading
 * `includes_all(robot.capabilities, {__ref: subject.required_capabilities})` — the example in this
 * module's own documentation — **passed when the policy's capability list was missing**. A rule
 * that passes because the fact is absent is worse than a rule that fails, because nobody goes
 * looking for it.
 *
 * `exists` is the escape hatch and always was: it is the one operator whose job is to ask whether a
 * path resolves, so it answers `unsatisfied` rather than `unevaluable` for an absent path, and
 * `not(exists(p))` still means what its author meant. An author who wants "absent counts as false"
 * writes it: `and(exists(p), includes(p, x))` — which is what the built-in rules already do.
 *
 * ── Total, this time for real ───────────────────────────────────────────────
 *
 * The paragraph at the top of this file has claimed totality since it was written, and four
 * malformed shapes threw instead: a `path` that is not a string reached `path.split`, and a
 * `predicates` that is not an array reached `.every` / `.some`. Rules are operator-supplied rows in
 * a database, `evaluateCompatibility` has no `try`, so one bad row took the compatibility check
 * down for every user of the platform — the exact failure totality was claimed to prevent. Both
 * shapes are now read defensively and answer `unevaluable`.
 */

import { isValidRange, isValidVersion, satisfies } from './semver.ts'
import type { Predicate } from './types.ts'

/**
 * Read `a.b.c` out of a context object. Returns undefined for any missing link.
 *
 * **Members only.** An array segment must be a decimal index, so `capabilities.length` does not
 * resolve: `length` is a property of a JavaScript array rather than a member of the JSON document,
 * and an edition without JavaScript arrays could not reproduce it. The same restriction keeps a
 * path off the prototype chain, where `constructor` and `__proto__` live and where a path composed
 * by somebody else has no business going.
 */
export function readPath(context: unknown, path: string | undefined): unknown {
  if (typeof path !== 'string' || path === '') return undefined
  let current: unknown = context
  for (const segment of path.split('.')) {
    if (current === null || current === undefined) return undefined
    if (typeof current !== 'object') return undefined
    if (Array.isArray(current)) {
      // A canonical decimal index, which is what a JSON array member is named.
      if (!/^(0|[1-9]\d*)$/.test(segment)) return undefined
      current = current[Number(segment)]
      continue
    }
    if (!Object.prototype.hasOwnProperty.call(current, segment)) return undefined
    current = (current as Record<string, unknown>)[segment]
  }
  return current
}

/**
 * Rule values may reference the context instead of holding a literal:
 *   { "__ref": "subject.required_capabilities" }
 * That is what lets one rule express "robot capabilities must cover whatever this policy
 * declares" without the rule author knowing the policy.
 */
export function resolveValue(value: unknown, context: unknown): unknown {
  const ref = referencedPath(value)
  return ref === null ? value : readPath(context, ref)
}

/** The path a rule value references, or `null` if it is a literal. */
function referencedPath(value: unknown): string | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  if (typeof record.__ref === 'string') return record.__ref
  // `{ path: … }` is accepted as an alias so hand-written rules read naturally.
  if (typeof record.path === 'string' && Object.keys(record).length === 1) return record.path
  return null
}

/** The predicate's own path, or null when it has none this edition can read. */
function pathOf(predicate: Predicate): string | null {
  return typeof predicate.path === 'string' && predicate.path !== '' ? predicate.path : null
}

/**
 * The children of a composite, or `null` when `predicates` is present and is not a list.
 *
 * Absent and non-array are deliberately different answers. Absent is *no children*, and a
 * conjunction over no children is satisfied — a skill declaring no preconditions does not fail
 * them. A `predicates` that is a string is a **broken rule**, and answering "no children" there
 * would turn it into a pass, which is the failure this whole module is being changed to stop.
 */
function childrenOf(predicate: Predicate): Predicate[] | null {
  const children = predicate.predicates
  if (children === undefined || children === null) return []
  return Array.isArray(children) ? children : null
}

function toArray(value: unknown): unknown[] {
  if (Array.isArray(value)) return value
  if (value === null || value === undefined) return []
  return [value]
}

function toNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

// ── The three-valued outcome ─────────────────────────────────────────────────

/**
 * Why a predicate could not be decided.
 *
 * A closed vocabulary, because §54.2 requires the refusal to *name* something and a free-text
 * reason is the same nothing as a free-text precondition. `corpus/vocabulary/closed-sets.json`
 * holds every edition to this list.
 */
export type UnevaluableReason =
  /** Not an object, or `op` is not a string. There is no predicate here. */
  | 'malformed'
  /** An operator this edition does not implement. */
  | 'unknown_op'
  /** The predicate names no path to read. */
  | 'missing_path'
  /** `not` with nothing to negate. */
  | 'missing_operand'
  /** A path that names nothing in this context. This is the one §54.2 is about. */
  | 'unresolved_path'
  /** An operand is present but is not a finite number, and the operator compares numbers. */
  | 'not_a_number'
  /** An operand is present but is not a version, or not a range. */
  | 'not_a_version'

export type Outcome = 'satisfied' | 'unsatisfied' | 'unevaluable'

export interface PredicateOutcome {
  outcome: Outcome
  /** Why, when `unevaluable`. Null otherwise. */
  reason: UnevaluableReason | null
  /** The path that could not be resolved or read, when there is one. Null otherwise. */
  path: string | null
  /** The operator that could not decide. Null when there was no readable operator at all. */
  op: string | null
}

export const OUTCOMES: Outcome[] = ['satisfied', 'unsatisfied', 'unevaluable']

export const UNEVALUABLE_REASONS: UnevaluableReason[] = [
  'malformed', 'unknown_op', 'missing_path', 'missing_operand', 'unresolved_path',
  'not_a_number', 'not_a_version',
]

export const PREDICATE_OPS: string[] = [
  'and', 'or', 'not',
  'eq', 'neq', 'in', 'gte', 'lte', 'gt', 'lt',
  'includes', 'includes_all', 'includes_any', 'exists', 'semver_satisfies',
]

function decided(value: boolean): PredicateOutcome {
  return { outcome: value ? 'satisfied' : 'unsatisfied', reason: null, path: null, op: null }
}

function unevaluable(reason: UnevaluableReason, path: string | null, op: string | null): PredicateOutcome {
  return { outcome: 'unevaluable', reason, path, op }
}

/** Whether an outcome permits whatever it was guarding. Only `satisfied` does. */
export function satisfied(outcome: PredicateOutcome): boolean {
  return outcome.outcome === 'satisfied'
}

/**
 * One side of a comparison.
 *
 * `path` is where the value came from, or null for a literal, and it is load-bearing twice: it is
 * what a refusal names, and it is how [`identical`] decides object equality without a reference to
 * compare.
 */
interface Operand {
  value: unknown
  path: string | null
  /** False when the path named nothing. A literal is always resolved, even a literal `null`. */
  resolved: boolean
}

function fromPath(context: unknown, path: string): Operand {
  const value = readPath(context, path)
  return { value, path, resolved: value !== undefined }
}

function fromValue(value: unknown, context: unknown): Operand {
  const ref = referencedPath(value)
  if (ref === null) return { value, path: null, resolved: true }
  return fromPath(context, ref)
}

/** An element of a collection operand, which is a member of the context in its own right. */
function element(collection: Operand, item: unknown, index: number): Operand {
  return {
    value: item,
    path: collection.path === null ? null : `${collection.path}.${index}`,
    resolved: true,
  }
}

/** The first operand that named nothing, as a refusal. Null when they all resolved. */
function firstUnresolved(op: string, ...operands: Operand[]): PredicateOutcome | null {
  for (const operand of operands) {
    if (!operand.resolved) return unevaluable('unresolved_path', operand.path, op)
  }
  return null
}

/**
 * JavaScript's `===` on two objects is reference equality, and the reference is the specification,
 * so the rule other editions must reproduce is written here rather than left to the host language.
 *
 * Two operands are identical when:
 *
 * * both are primitives — `undefined`, `null`, a boolean, a number or a string — and equal; or
 * * both were read from the **same path of the same context**, which is the only way two
 *   non-primitive operands can be one object.
 *
 * A literal object or array in a rule is therefore never equal to anything, which is exactly what
 * `===` does today and is worth knowing before writing `{"op":"eq","value":["a"]}` and wondering
 * why it never fires. `includes` is the operator for that.
 *
 * A context whose object graph aliases — two different paths reaching one object — is outside the
 * specified domain, because a context that arrived as JSON cannot express it.
 */
function identical(left: Operand, right: Operand): boolean {
  const primitive = (value: unknown) => value === null || typeof value !== 'object'
  if (primitive(left.value) && primitive(right.value)) return left.value === right.value
  if (left.path === null || right.path === null) return false
  return left.path === right.path
}

/** Membership, with [`identical`]'s rule applied element by element. */
function contains(collection: Operand, needle: Operand): boolean {
  return toArray(collection.value).some((item, index) => identical(element(collection, item, index), needle))
}

/**
 * The evaluator. Everything else in this file is a view of it.
 */
export function evaluatePredicateOutcome(
  predicate: Predicate | null | undefined,
  context: unknown,
): PredicateOutcome {
  if (!predicate || typeof predicate !== 'object' || typeof predicate.op !== 'string') {
    return unevaluable('malformed', null, null)
  }

  const op = predicate.op

  switch (op) {
    // Order is part of the answer for all three: a refusal names *a* predicate, and which one it
    // names must not depend on how an edition iterates.
    case 'and': {
      const children = childrenOf(predicate)
      if (children === null) return unevaluable('malformed', null, op)
      for (const child of children) {
        const outcome = evaluatePredicateOutcome(child, context)
        if (outcome.outcome !== 'satisfied') return outcome
      }
      // Nothing was asked for. A skill declaring no preconditions is not a skill that fails them.
      return decided(true)
    }

    case 'or': {
      const children = childrenOf(predicate)
      if (children === null) return unevaluable('malformed', null, op)
      let firstUnevaluable: PredicateOutcome | null = null
      for (const child of children) {
        const outcome = evaluatePredicateOutcome(child, context)
        if (outcome.outcome === 'satisfied') return outcome
        if (outcome.outcome === 'unevaluable' && firstUnevaluable === null) firstUnevaluable = outcome
      }
      // An alternative that held would have returned already; an alternative nobody could read
      // means this disjunction is not known to be false, only unread.
      return firstUnevaluable ?? decided(false)
    }

    case 'not': {
      const children = childrenOf(predicate)
      if (children === null) return unevaluable('malformed', null, op)
      const child = predicate.predicate ?? children[0]
      if (child === undefined) return unevaluable('missing_operand', null, op)
      const outcome = evaluatePredicateOutcome(child, context)
      // Ignorance does not negate into knowledge, and this is the line that used to say it did.
      if (outcome.outcome === 'unevaluable') return outcome
      return decided(outcome.outcome === 'unsatisfied')
    }

    case 'exists': {
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      // The one operator that answers *about* resolution, so it never refuses for lack of it.
      const value = readPath(context, path)
      if (value === null || value === undefined) return decided(false)
      if (Array.isArray(value)) return decided(value.length > 0)
      if (typeof value === 'string') return decided(value.trim() !== '')
      return decided(true)
    }

    case 'eq': case 'neq': {
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      const left = fromPath(context, path)
      const right = fromValue(predicate.value, context)
      const refused = firstUnresolved(op, left, right)
      if (refused) return refused
      const equal = identical(left, right)
      return decided(op === 'eq' ? equal : !equal)
    }

    case 'in': {
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      const needle = fromPath(context, path)
      const collection = fromValue(predicate.value, context)
      const refused = firstUnresolved(op, needle, collection)
      if (refused) return refused
      return decided(contains(collection, needle))
    }

    case 'includes': {
      // The context value is a collection that must contain the rule's value.
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      const collection = fromPath(context, path)
      const needle = fromValue(predicate.value, context)
      const refused = firstUnresolved(op, collection, needle)
      if (refused) return refused
      return decided(contains(collection, needle))
    }

    case 'includes_all': case 'includes_any': {
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      const collection = fromPath(context, path)
      const wanted = fromValue(predicate.value, context)
      const refused = firstUnresolved(op, collection, wanted)
      if (refused) return refused
      const items = toArray(wanted.value)
      const each = items.map((item, index) => contains(collection, element(wanted, item, index)))
      // An empty requirement is vacuously satisfied — nothing was asked for. Note that this is
      // now reachable only from a requirement that *is* empty, never from one nobody could read.
      if (op === 'includes_any') return decided(items.length === 0 || each.some(Boolean))
      return decided(each.every(Boolean))
    }

    case 'gte': case 'lte': case 'gt': case 'lt': {
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      const leftOperand = fromPath(context, path)
      const rightOperand = fromValue(predicate.value, context)
      const refused = firstUnresolved(op, leftOperand, rightOperand)
      if (refused) return refused
      const left = toNumber(leftOperand.value)
      if (left === null) return unevaluable('not_a_number', leftOperand.path, op)
      const right = toNumber(rightOperand.value)
      if (right === null) return unevaluable('not_a_number', rightOperand.path, op)
      if (op === 'gte') return decided(left >= right)
      if (op === 'lte') return decided(left <= right)
      if (op === 'gt') return decided(left > right)
      return decided(left < right)
    }

    case 'semver_satisfies': {
      const path = pathOf(predicate)
      if (path === null) return unevaluable('missing_path', null, op)
      const versionOperand = fromPath(context, path)
      const rangeOperand = fromValue(predicate.value, context)
      const refused = firstUnresolved(op, versionOperand, rangeOperand)
      if (refused) return refused
      const version = versionOperand.value
      const range = rangeOperand.value
      // A version this edition cannot parse is a fact about the declaration, not a version that
      // fell outside the window — so it is named rather than counted as a miss.
      if (typeof version !== 'string' || !isValidVersion(version)) {
        return unevaluable('not_a_version', versionOperand.path, op)
      }
      if (typeof range !== 'string' || !isValidRange(range)) {
        return unevaluable('not_a_version', rangeOperand.path, op)
      }
      return decided(satisfies(version, range))
    }

    default:
      return unevaluable('unknown_op', pathOf(predicate), op)
  }
}

/**
 * The compatibility engine's answer: does this rule hold?
 *
 * Derived, not reimplemented. See the header for the five inputs whose answer this changed and
 * why all five moved the same way.
 */
export function evaluatePredicate(predicate: Predicate | null | undefined, context: unknown): boolean {
  return satisfied(evaluatePredicateOutcome(predicate, context))
}

/**
 * Values a rule looked at, so a failing finding can show the user *why* rather than just
 * asserting incompatibility. Collects every `path` mentioned anywhere in the predicate tree.
 */
export function collectPredicateDetail(predicate: Predicate | null | undefined, context: unknown): Record<string, unknown> {
  const detail: Record<string, unknown> = {}

  const walk = (node: Predicate | null | undefined) => {
    if (!node || typeof node !== 'object') return
    const path = pathOf(node)
    if (path !== null) detail[path] = readPath(context, path)
    // A `__ref` value is just as much part of the comparison as the left-hand path.
    const ref = referencedPath(node.value)
    if (ref !== null) detail[ref] = readPath(context, ref)
    if (node.predicate) walk(node.predicate)
    for (const child of childrenOf(node) ?? []) walk(child)
  }

  walk(predicate)
  return detail
}
