/**
 * §54.2's live precondition context, assembled.
 *
 * §54.2 turns `SkillManifest.preconditions` from prose into predicates and then names the seven
 * branches those predicates may read:
 *
 * ```
 * robot.state.*        joint positions, gripper, battery percent, base pose
 * robot.guards.active  which guards are actually running (§7.2)
 * robot.calibration.*  which parameter sets are valid, and their age (§45)
 * robot.frames.*       whether the frames the skill needs resolve (§44)
 * robot.tier           guest | host | certified
 * skill.grants.*       capability tokens the operator granted this package
 * env.*                operator-declared facts about the cell — fixture present, door state
 * ```
 *
 * `evaluate` in `predicate.ts` has existed since the compatibility engine did, and
 * `evaluatePreconditions` in `skill.ts` since §54 was written. Both take the context as an
 * argument, and until now nothing built one. That is the gap this file closes, and it is a
 * **contract** rather than daemon code for the reason every other file here is one: the path a
 * skill publisher writes into a predicate is a promise the OS makes about what it will supply, and
 * a promise assembled ad hoc inside a daemon is undocumented, untested, and different in the next
 * edition.
 *
 * # The rule that decides every field: absent is not null
 *
 * §54.2's first rule is the one with teeth:
 *
 * > **A precondition that cannot be evaluated refuses the skill**, naming the path that was
 * > unresolvable. Not "assume true", which is what free-text preconditions do by construction.
 *
 * So a fact this robot does not know must be **absent** from the context, never present as `null`
 * and never present as a plausible zero. A robot with no battery sensor that reported
 * `robot.state.battery_percent = 0` would refuse every skill; one that reported `100` would run
 * every skill; one that omits the path refuses only the skills that asked, and names the path in
 * the refusal. Only the third is honest, and only the third is useful.
 *
 * # Two branches are lists rather than objects, and the reason is a dot
 *
 * `cell.ts` found, from the other side, that a fact key containing a dot is a fact no precondition
 * can address: `{"door.closed": true}` and `{"door": {"closed": true}}` are the same path and
 * different documents. It answers by *refusing* such a key, because an operator can rename a fact.
 *
 * A **capability token contains a dot by construction** — `arm.left`, `sensing.camera`,
 * `locomotion.base` — so `skill.grants.<token>` is unaddressable for every token that exists, and
 * nobody can rename them. The shape therefore has to change rather than the names: grants are a
 * **sorted list** at `skill.grants.tokens`, read with `includes` and `includes_all`, which the
 * predicate language has had from the start and which is exactly the operation a precondition about
 * grants wants anyway.
 *
 * Frames are a list for the second half of the same reason. §44 lets a frame id carry a namespace,
 * and §54.2's question about them is *"whether the frames the skill needs resolve"* — a containment
 * question, not a lookup. `robot.frames.resolved` with `includes_all` asks it in one clause; an
 * object keyed by frame id would ask it in one clause per frame and get the empty case wrong.
 *
 * Joint names are *not* moved to a list, because the question about a joint is its **value** and a
 * list cannot answer that. They keep `robot.state.joints.<name>`, and a joint whose name a dot path
 * cannot address is omitted and **reported** — see `LiveContext.unaddressable`. That is the one
 * asymmetry here and it is the honest one: the OS cannot rename a body's joints, so it says which
 * ones no precondition will ever be able to name rather than pretending the branch is complete.
 *
 * # Pure
 *
 * No clock, no I/O. Ages arrive already computed from a supplied wall-clock reading, for the reason
 * the kernel takes `now_ms` as a parameter (§35.2), and every other field arrives already observed.
 */

import type { Validity } from './calibration.ts'
import type { FrameFault } from './embodiment.ts'
import type { OperatingState } from './opstate.ts'

/** A joint's measured position, in the units its channel declares. */
export interface JointReading {
  name: string
  /** Non-finite readings are dropped rather than published — see `LiveContext.unreadable`. */
  position: number
}

/** What is known about one calibration set (§45), flattened for a precondition to read. */
export interface CalibrationFact {
  set: string
  validity: Validity
  /**
   * Seconds since it was taken, when both the record and a clock reading were available.
   *
   * Absent omits the path rather than reporting zero: §54.2 asks for *"which parameter sets are
   * valid, and their age"*, and a robot with no clock does not know the age of anything.
   */
  age_s?: number | null
}

/**
 * Where the base is, when a driver reports it.
 *
 * Position and orientation in one object because §54.2 names *"base pose"* as one fact, and a
 * context that could publish a position with no orientation would let a precondition about a docked
 * robot read true while it faced the wrong way.
 */
export interface BasePose {
  /** Metres, in the body's world frame. */
  xyz: [number, number, number]
  /** Unit quaternion, `x, y, z, w` — the order §44 uses everywhere. */
  quaternion: [number, number, number, number]
}

/**
 * Everything the OS knows, as the arguments the context is built from.
 *
 * Every field that can be unknown is optional or an empty collection, and every one of those omits
 * its path rather than filling it. That is `liveContext`'s whole contract with §54.2's first rule.
 */
export interface LiveInputs {
  joints?: JointReading[]
  /** The gripper's opening, 0 closed to 1 open, when the body has one. */
  gripper?: number | null
  battery_percent?: number | null
  base_pose?: BasePose | null
  /**
   * Where §27's machine says the robot is.
   *
   * Optional even though the daemon always has one, because a context assembled by something that
   * has not observed the machine must not claim `post` by default — that is a state, and stating it
   * would let a precondition about the operating state read a value nobody measured.
   */
  operating_state?: OperatingState | null
  /**
   * Which of §7.2's eight guards are actually evaluating.
   *
   * Absent and `[]` are different facts and the distinction is the whole module in one field: an
   * empty list is a kernel reporting that no guard is running, and absent is nobody having asked a
   * kernel. §33.1 keeps `skynetd` on the far side of that line, so a daemon with no kernel channel
   * supplies nothing — an empty list from it would be a claim rather than an observation.
   */
  guards_active?: string[] | null
  /** Which are not, and are therefore reporting unavailable (§45.5). */
  guards_unavailable?: string[]
  calibration?: CalibrationFact[]
  /** Frame ids that resolve against the flattened graph (§44). */
  frames_resolved?: string[]
  /**
   * Why the graph does not flatten, when it does not.
   *
   * The frame validator's own fault rather than a sentence, so a precondition can name the
   * **code** — *"refuse when the graph has a cycle"* is a clause somebody will write, and a string
   * forces them to match on prose. §64's rule that every refusal names the fix is checked
   * mechanically over the corpus, and a bare string under a key called `fault` is exactly the
   * shape that check refuses.
   */
  frames_fault?: FrameFault | null
  tier?: string | null
  /** The capability tokens the operator granted **this package** (§8, §38). */
  grants?: string[]
  /**
   * The cell's `env.*` branch, from `cellEnvironment`.
   *
   * Taken already-built rather than as a `CellDescriptor`, because §55.2's document has one other
   * consumer — the `workspace` guard — and a robot with no cell profile at all still has the other
   * six branches.
   */
  env?: Record<string, unknown> | null
}

/** The context, plus what it could not say and why. */
export interface LiveContext {
  /** The document a predicate reads. */
  context: Record<string, unknown>
  /**
   * Joint and set names no dot path can address, so no precondition can ever name them.
   *
   * Reported rather than silently dropped, because the symptom otherwise is a precondition that
   * refuses with `robot.state.joints.arm.1` unresolvable and an operator who can see the joint on
   * the console.
   */
  unaddressable: string[]
  /**
   * Readings that were not finite, and are therefore not readings.
   *
   * A NaN joint is the fault `drivers/mock` injects and §7.2's envelope guard refuses on. It must
   * not reach a precondition as a number, and it must not reach one as a missing joint with no
   * explanation either.
   */
  unreadable: string[]
}

/**
 * A key a dot path can address: non-empty, no dot, and no leading or trailing whitespace.
 *
 * The same rule as `cell.ts`'s, deliberately duplicated rather than shared, because they are two
 * rules that happen to agree: this one is about a name the **body** chose and that one is about a
 * name the **operator** chose, and the two could reasonably diverge about whitespace without either
 * being wrong.
 */
function addressable(key: string): boolean {
  return key.length > 0 && !key.includes('.') && key.trim() === key
}

function finite(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

function strList(values: readonly string[]): string[] {
  return [...new Set(values)].sort()
}

/**
 * Build §54.2's context.
 *
 * Every branch is omitted rather than emitted empty when nothing is known about it, with one
 * deliberate exception: `skill.grants.tokens` is emitted as an empty array.
 *
 * The reason is that a grant is a **file the daemon owns**. "This package was granted nothing" is an
 * answer the OS always has, and a precondition should be able to refuse on it and say so. Every
 * other branch is something the OS learns from somewhere else — a driver, a kernel, a clock, an
 * operator's survey of a room — and any of them can be missing, at which point §54.2's first rule is
 * the honest answer and an empty array is a claim.
 */
export function liveContext(inputs: LiveInputs): LiveContext {
  const unaddressable: string[] = []
  const unreadable: string[] = []

  // ── robot.state ────────────────────────────────────────────────────────────
  const joints: Record<string, number> = {}
  let anyJoint = false
  for (const reading of inputs.joints ?? []) {
    if (!addressable(reading.name)) {
      unaddressable.push(reading.name)
      continue
    }
    if (!finite(reading.position)) {
      unreadable.push(reading.name)
      continue
    }
    joints[reading.name] = reading.position
    anyJoint = true
  }

  const state: Record<string, unknown> = {}
  if (anyJoint) state.joints = joints
  if (finite(inputs.gripper)) state.gripper = inputs.gripper
  if (finite(inputs.battery_percent)) state.battery_percent = inputs.battery_percent
  const pose = inputs.base_pose
  if (pose) {
    if (pose.xyz.every(finite) && pose.quaternion.every(finite)) {
      state.base_pose = {
        x: pose.xyz[0], y: pose.xyz[1], z: pose.xyz[2],
        qx: pose.quaternion[0], qy: pose.quaternion[1],
        qz: pose.quaternion[2], qw: pose.quaternion[3],
      }
    } else {
      unreadable.push('base_pose')
    }
  }
  // §54.2's table says `robot.state.*` and then lists four examples; §27's state is inside that
  // branch and is the fact a precondition most obviously wants — *"only when the robot is running"*
  // is the first precondition anybody writes. It is published under its own name rather than inside
  // a status object so the path stays stable if §27 ever grows a second field.
  if (inputs.operating_state) state.operating = inputs.operating_state

  // ── robot.guards ───────────────────────────────────────────────────────────
  const guards: Record<string, unknown> = {}
  if (inputs.guards_active) guards.active = strList(inputs.guards_active)
  if ((inputs.guards_unavailable ?? []).length > 0) {
    guards.unavailable = strList(inputs.guards_unavailable ?? [])
  }

  // ── robot.calibration ──────────────────────────────────────────────────────
  const calibration: Record<string, unknown> = {}
  let anyCalibration = false
  for (const fact of inputs.calibration ?? []) {
    if (!addressable(fact.set)) {
      unaddressable.push(fact.set)
      continue
    }
    const row: Record<string, unknown> = {
      valid: fact.validity.state === 'valid',
      state: fact.validity.state,
    }
    if (finite(fact.age_s)) row.age_s = fact.age_s
    calibration[fact.set] = row
    anyCalibration = true
  }

  // ── robot ──────────────────────────────────────────────────────────────────
  const robot: Record<string, unknown> = {}
  if (Object.keys(state).length > 0) robot.state = state
  if (Object.keys(guards).length > 0) robot.guards = guards
  if (anyCalibration) robot.calibration = calibration
  const frames: Record<string, unknown> = {}
  if ((inputs.frames_resolved ?? []).length > 0) {
    frames.resolved = strList(inputs.frames_resolved ?? [])
  }
  if (inputs.frames_fault) {
    frames.fault = {
      code: inputs.frames_fault.code,
      detail: inputs.frames_fault.detail,
      subjects: strList(inputs.frames_fault.subjects),
    }
  }
  if (Object.keys(frames).length > 0) robot.frames = frames
  if (typeof inputs.tier === 'string') robot.tier = inputs.tier

  // ── skill.grants ───────────────────────────────────────────────────────────
  const skill = { grants: { tokens: strList(inputs.grants ?? []) } }

  const context: Record<string, unknown> = { robot, skill }
  if (inputs.env) context.env = inputs.env

  return { context, unaddressable: strList(unaddressable), unreadable: strList(unreadable) }
}

/**
 * What a console shows about a context it could not fully build.
 *
 * One sentence per omission, naming the path that will be unresolvable, because §54.2's refusal
 * names a path and an operator has to be able to get from that path back to the reason.
 */
export function explainOmissions(built: LiveContext): string[] {
  const lines: string[] = []
  for (const name of built.unaddressable) {
    lines.push(`\`${name}\` carries a dot, so no precondition can address it — a dot path would `
      + 'read the name as two keys. Nothing can be done about that here: the body chose the name.')
  }
  for (const name of built.unreadable) {
    lines.push(`\`${name}\` did not report a finite value, so it is absent rather than published. `
      + '§54.2 refuses a precondition that names it, which is the honest answer to a sensor '
      + 'reporting NaN.')
  }
  return lines
}
