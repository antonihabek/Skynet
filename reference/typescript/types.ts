/**
 * The one type the reference modules import from outside themselves.
 *
 * On the Hub this lives in `lib/skynet/types.ts` alongside the rest of the marketplace's object
 * model. Here it is restated so the oracle is standalone: `node reference/typescript/*.ts` must
 * run with nothing else installed and nothing else present, because a corpus generator that
 * needs a Next.js application checked out beside it is a corpus generator nobody runs.
 */

export interface SafetyEnvelope {
  /** Per-joint angular velocity ceiling. */
  joint_velocity_max_rad_s?: number
  /** Per-joint torque ceiling. */
  joint_torque_max_nm?: number
  /** Whole-body translation ceiling. */
  base_velocity_max_m_s?: number
  /** Command timeout before the controller faults, in milliseconds. */
  watchdog_ms?: number
  /** Whether a hardware emergency stop is fitted. */
  estop?: boolean
  /** Where the figures came from — a datasheet URL. A limit with no provenance is a rumour. */
  source?: string
  /**
   * What kind of figures these are (§5.3): `declared` transcribed from documentation, `measured`
   * taken on the body. Absent reads as `declared` — every envelope written before this field is
   * a declaration. Envelope-scoped, and not §87's safe-state `basis`.
   */
  basis?: string
}

/**
 * The predicate language (§54.2).
 *
 * Restated here for the same reason `SafetyEnvelope` is: on the Hub this lives in
 * `lib/skynet/types.ts` beside the rest of the object model, and the oracle has to stand alone.
 *
 * The vocabulary is closed and `corpus/vocabulary/closed-sets.json` holds every edition to it —
 * `predicate.ts` exports the same list as `PREDICATE_OPS`, and this type is the compiler's view of
 * it rather than a second definition.
 */
export type PredicateOp =
  | 'and' | 'or' | 'not'
  | 'eq' | 'neq' | 'in' | 'gte' | 'lte' | 'gt' | 'lt'
  | 'includes' | 'includes_all' | 'includes_any' | 'exists' | 'semver_satisfies'

export interface Predicate {
  op: PredicateOp
  /** Dot path into the context, e.g. "robot.guards.active". */
  path?: string
  value?: unknown
  predicates?: Predicate[]
  predicate?: Predicate
}
