/**
 * The cell descriptor (§55.2) — the physical counterpart of an environment.
 *
 * §55.1 makes an environment a simulation artifact that only a simulator driver can load. §55.2
 * names its real-world analogue and then refuses to make it a repo kind:
 *
 * > The real-world analogue of an environment — where the fixtures are, which volumes are
 * > forbidden, where the safe parking pose is, what the operator has declared about the space — is
 * > a **cell descriptor**, and it belongs in the operator profile (§38), not in the repository
 * > catalogue.
 * >
 * > The reason is ownership. An environment is published, versioned and shared; a cell is
 * > site-specific, changes when somebody moves a table, and is nobody's intellectual property.
 * > Making it a repo kind would invite people to publish their factory layout to a marketplace, and
 * > would put safety-relevant workspace volumes behind a fork-and-star workflow.
 *
 * So this file has no publishing, no versioning and no digest. It is a profile document, signed
 * like every other safety-relevant profile (§38.1), and it does exactly two jobs:
 *
 * * it supplies **`env.*`** to the precondition context (§54.2), which is what lets a skill say
 *   "the cell door is closed" and have the OS check it;
 * * it supplies **allowed volumes** to the `workspace` guard (§7.1's fourth), as one layer of
 *   §38.2's stack, where layers compose by conjunction and can therefore only narrow.
 *
 * ── The check that only exists because §54.2 does ──────────────────────────
 *
 * `env.*` is addressed by a **dot path**, so a fact whose key contains a dot is a fact no
 * precondition can ever name: `{"door.closed": true}` and `{"door": {"closed": true}}` are the same
 * path and different documents, and the first will never match. That is not a hypothetical typo —
 * it is the obvious way to write a hierarchical key for anybody who has not read `readPath`.
 *
 * Nothing could have caught it before this session, because until §54.2 there was no path
 * vocabulary for it to be wrong in. [`validateCell`] refuses it, and refuses a key that would
 * shadow one of the derived facts below, because a cell that silently overwrote its own name would
 * be a cell whose preconditions read true for the wrong reason.
 *
 * ── The third job, and it is a refusal (§87.1) ─────────────────────────────
 *
 * §43 left *"multiple robots on one compute"* open for four sessions and §87.1 answers it: **one
 * kernel, one driver and one black box per robot.** What that decision costs is a sentence worth
 * carrying here, because this file is where the cost is paid — **two kernels cannot see each
 * other's envelope**, so a constraint over the *pair* of robots is enforced by neither of them.
 *
 * The platform's answer is not to invent a cell envelope nobody evaluates. It is to refuse the
 * claim it cannot enforce: [`CellDescriptor.robots`] places each robot and draws where it may be,
 * and a cell whose robots share a point — or one that has not said enough for anybody to know — is
 * reported with the pair named. That is the same move `validateCell` already makes about a fact key
 * nobody can address, one level up: the check exists because the document is now able to say
 * something the platform would otherwise have to pretend about.
 */

import type { Shape, Vec3, Volume, WorkspaceLayer } from './kinematics.ts'

/** A thing in the cell that is not the robot. */
export interface Fixture {
  name: string
  /** Where it is, when the operator has drawn it. Fixtures may be named without being measured. */
  volume?: Volume | null
}

/**
 * One robot in a cell that holds more than one (§87.1).
 *
 * §87.1's decision is **one kernel, one driver and one black box per robot**, and the cost it names
 * is the reason this interface exists: two kernels cannot see each other's envelope, so a constraint
 * over the *pair* is enforced by neither. The platform's answer is not a cell envelope nobody
 * evaluates — it is to refuse the claim it cannot enforce, and that refusal needs the operator to
 * have said where the robots are and where each may be.
 */
export interface CellRobot {
  /** The robot, as the Hub and the descriptor name it. A precondition reads it as `env.robot`. */
  slug: string
  /**
   * Where this robot's root frame sits in the cell's own frame, metres.
   *
   * §44.2 puts the safety frame set at the robot's root, so a volume in [`CellRobot.allowed`] is in
   * *that* robot's coordinates. Two robots' volumes therefore cannot be compared at all until
   * somebody says where the robots are, which is §5.2 one level up: a disjointness claim with no
   * placement is a rumour. Required from the second robot, and meaningless before it.
   */
  at?: Vec3 | null
  /**
   * Where this robot may be, in its own root frame.
   *
   * One workspace layer named `cell:<cell>:<slug>`, composing with the cell's own top-level
   * `allowed` by §38.2's conjunction — so a robot's layer can only narrow what the cell allowed
   * everything.
   */
  allowed?: Volume[]
}

/** One joint of the pose the robot parks in. */
export interface ParkingChannel {
  /** The command channel, in the body's own vocabulary — the same names a command cycle carries. */
  channel: string
  position: number
}

/**
 * What an operator has written down about one physical space.
 *
 * Deliberately small. Everything here is either checkable by the OS or readable by a precondition;
 * a field nobody consumes is a field that decays.
 */
export interface CellDescriptor {
  /** Site-local, and the only identity a cell has. Never published, never versioned. */
  name: string
  /** Free facts about the space, reaching preconditions as `env.<key>`. */
  facts?: Record<string, unknown>
  /** Where the robot may be. One layer of §38.2's stack; layers compose by conjunction. */
  allowed?: Volume[]
  fixtures?: Fixture[]
  /** Where the robot parks when a skill ends or is stopped (§48). */
  parking_pose?: ParkingChannel[]
  /**
   * The robots in this cell (§87.1).
   *
   * **Absent, empty and one robot are the same cell** — the one §55.2 already describes — so every
   * cell written before §87 is unchanged. The placement and disjointness rules apply only from the
   * second robot, because with one robot there is no pair and the cell *is* its frame.
   */
  robots?: CellRobot[]
}

/**
 * Facts this file derives, which a cell's own `facts` may not shadow.
 *
 * A precondition reading `env.cell` must be reading the cell's name, on every robot, whatever
 * somebody typed into their configuration. `robot` and `robots` join them at §87.1, and
 * `env.robot` is what makes that class gate usable from a skill: *"I am the only robot in this
 * cell"* is a precondition somebody can write, and it is the one a skill wants on a machine whose
 * platform has just told it that nothing evaluates the pair.
 */
export const DERIVED_FACTS = ['cell', 'fixtures', 'robot', 'robots'] as const

export const CELL_FAULTS = [
  'unnamed',
  'fact_key_unusable',
  'fact_key_reserved',
  'volume_frame_missing',
  'volume_shape_invalid',
  'fixture_unnamed',
  'duplicate_fixture',
  'parking_channel_unnamed',
  'duplicate_parking_channel',
  // §87.1, and all five apply only from the second robot.
  'robot_unnamed',
  'duplicate_robot',
  'robot_unplaced',
  'robot_unbounded',
  'shared_volume',
] as const

export type CellFaultCode = (typeof CELL_FAULTS)[number]

export interface CellFault {
  code: CellFaultCode
  /** What is wrong, by name — a key, a fixture, a channel. */
  subjects: string[]
  detail: string
}

/**
 * The cell's name, or the empty string.
 *
 * A document whose `name` is missing or is not a string has no name, and the empty string is how
 * both editions spell that — `null` here where the Rust reader defaults to `""` would be a
 * divergence in one byte of one field, found by a corpus case and not by anybody reading either
 * file.
 */
function cellName(cell: CellDescriptor): string {
  return typeof cell.name === 'string' ? cell.name : ''
}

/** A key a dot path can address: non-empty, no dot, and no leading or trailing whitespace. */
function usableKey(key: string): boolean {
  return key.length > 0 && !key.includes('.') && key.trim() === key
}

function shapeIsUsable(volume: Volume): boolean {
  const shape = volume.shape
  if (!shape || typeof shape !== 'object') return false
  if (shape.kind === 'sphere') {
    return Number.isFinite(shape.radius_m) && shape.radius_m > 0
      && shape.centre.every(value => Number.isFinite(value))
  }
  if (shape.kind === 'box') {
    if (!shape.min.every(Number.isFinite) || !shape.max.every(Number.isFinite)) return false
    // A box whose min exceeds its max on any axis is empty, and an empty allowed volume is a cell
    // the robot may never be in — which is a configuration mistake rather than a lock-out to
    // enforce silently.
    return shape.min.every((value, axis) => value <= shape.max[axis])
  }
  return false
}

// ── §87.1's intersection test ────────────────────────────────────────────────

/**
 * A shape moved into the cell's frame by its robot's placement.
 *
 * Translation only. §44.4's transforms carry a rotation and this deliberately does not read one: a
 * rotated box is not a box, so the intersection test would need an oriented-box routine whose
 * disagreements between eight editions would be in the last bits of a square root. A cell whose
 * robots are mounted at an angle is a cell whose operator draws axis-aligned volumes large enough
 * to hold the rotation, which over-reports sharing and never under-reports it — the direction that
 * fails safe.
 */
function placed(shape: Shape, at: Vec3): Shape {
  if (shape.kind === 'sphere') {
    return {
      kind: 'sphere',
      centre: [shape.centre[0] + at[0], shape.centre[1] + at[1], shape.centre[2] + at[2]],
      radius_m: shape.radius_m,
    }
  }
  return {
    kind: 'box',
    min: [shape.min[0] + at[0], shape.min[1] + at[1], shape.min[2] + at[2]],
    max: [shape.max[0] + at[0], shape.max[1] + at[1], shape.max[2] + at[2]],
  }
}

/** Squared distance from a point to a box, zero inside. */
function boxPointDistanceSquared(box: { min: Vec3; max: Vec3 }, point: Vec3): number {
  let total = 0
  for (let axis = 0; axis < 3; axis += 1) {
    const value = point[axis]
    const low = box.min[axis]
    const high = box.max[axis]
    const outside = value < low ? low - value : value > high ? value - high : 0
    total += outside * outside
  }
  return total
}

/**
 * Whether two placed shapes share a point.
 *
 * **Inclusive, and that is the decision rather than an off-by-one.** `margin` counts zero as inside
 * — a workspace trip is `margin < 0` — so two robots whose boxes meet on a face can each legally be
 * at the point where they meet. A strict test would call that configuration disjoint and the
 * platform would have certified the one arrangement where both arms are allowed into the same
 * millimetre.
 */
function intersects(a: Shape, b: Shape): boolean {
  if (a.kind === 'box' && b.kind === 'box') {
    for (let axis = 0; axis < 3; axis += 1) {
      if (a.min[axis] > b.max[axis] || b.min[axis] > a.max[axis]) return false
    }
    return true
  }
  if (a.kind === 'sphere' && b.kind === 'sphere') {
    const dx = a.centre[0] - b.centre[0]
    const dy = a.centre[1] - b.centre[1]
    const dz = a.centre[2] - b.centre[2]
    const reach = a.radius_m + b.radius_m
    return dx * dx + dy * dy + dz * dz <= reach * reach
  }
  const box = a.kind === 'box' ? a : (b as Extract<Shape, { kind: 'box' }>)
  const sphere = a.kind === 'sphere' ? a : (b as Extract<Shape, { kind: 'sphere' }>)
  return boxPointDistanceSquared(box, sphere.centre) <= sphere.radius_m * sphere.radius_m
}

/** A robot's usable volumes, placed. A volume with no usable shape is not compared. */
function placedVolumes(robot: CellRobot): Array<{ frame: string; shape: Shape }> {
  const at = readPlacement(robot)
  if (at === null) return []
  return (robot.allowed ?? [])
    .filter(volume => shapeIsUsable(volume))
    .map(volume => ({ frame: volume.frame, shape: placed(volume.shape, at) }))
}

/** The placement, or null when the operator wrote none the arithmetic can use. */
function readPlacement(robot: CellRobot): Vec3 | null {
  const at = robot.at
  if (!Array.isArray(at) || at.length !== 3) return null
  return at.every(value => typeof value === 'number' && Number.isFinite(value))
    ? [at[0], at[1], at[2]]
    : null
}

/** A robot's slug, or the empty string — `cellName`'s rule, one level down. */
function robotSlug(robot: CellRobot): string {
  return typeof robot.slug === 'string' ? robot.slug : ''
}

/**
 * Every fault in a cell descriptor.
 *
 * Fact keys are walked in sorted order; everything else in the order it was written. See the
 * comment on the loop for why sorting is a compatibility decision rather than a tidy one.
 *
 * A **list** rather than the first one, unlike `validateFrames` next door, and the difference is
 * about who is reading. A frame graph that does not form a tree cannot be analysed further, so one
 * fault is all there is to say. A cell descriptor's problems are independent, an operator is
 * editing it by hand, and `admitPackage`'s reason applies unchanged: fixing four problems one
 * deployment at a time is how people end up giving up and granting everything.
 */
export function validateCell(cell: CellDescriptor): CellFault[] {
  const faults: CellFault[] = []

  if (typeof cell.name !== 'string' || cell.name.trim() === '') {
    faults.push({
      code: 'unnamed',
      subjects: [],
      detail: 'A cell has no identity but its name, and this one has none.',
    })
  }

  // Sorted, and that is a compatibility decision rather than a tidy one. The Rust edition holds a
  // parsed object in a `BTreeMap`, which has no insertion order to offer, so reporting in the order
  // somebody typed would make the two editions name the same faults in different orders. A
  // canonical JSON document's key order *is* sorted, so this is document order by the time either
  // edition sees one.
  for (const key of Object.keys(cell.facts ?? {}).sort()) {
    if (!usableKey(key)) {
      faults.push({
        code: 'fact_key_unusable',
        subjects: [key],
        detail: `A precondition addresses this as env.${key}, and a dot path cannot name it. `
          + 'Nest the object instead of putting a dot in the key.',
      })
      continue
    }
    if ((DERIVED_FACTS as readonly string[]).includes(key)) {
      faults.push({
        code: 'fact_key_reserved',
        subjects: [key],
        detail: `env.${key} is derived from the cell itself, so a fact of that name would be `
          + 'read by nobody and would make a precondition true for the wrong reason.',
      })
    }
  }

  cell.allowed?.forEach((volume, index) => {
    if (typeof volume.frame !== 'string' || volume.frame.trim() === '') {
      faults.push({
        code: 'volume_frame_missing',
        subjects: [`allowed[${index}]`],
        detail: 'A volume constrains a frame, and this one names none. The workspace guard would '
          + 'have nothing to check it against.',
      })
    }
    if (!shapeIsUsable(volume)) {
      faults.push({
        code: 'volume_shape_invalid',
        subjects: [`allowed[${index}]`],
        detail: 'This volume has no usable geometry — a box whose minimum exceeds its maximum, a '
          + 'sphere with no radius, or a shape this edition does not know.',
      })
    }
  })

  const fixtures = new Set<string>()
  cell.fixtures?.forEach((fixture, index) => {
    if (typeof fixture.name !== 'string' || fixture.name.trim() === '') {
      faults.push({
        code: 'fixture_unnamed',
        subjects: [`fixtures[${index}]`],
        detail: 'A fixture a precondition cannot name is a fixture the OS cannot check for.',
      })
      return
    }
    if (fixtures.has(fixture.name)) {
      faults.push({
        code: 'duplicate_fixture',
        subjects: [fixture.name],
        detail: 'Two fixtures with one name: a precondition asking for it would be asking about '
          + 'whichever the reader happened to reach.',
      })
      return
    }
    fixtures.add(fixture.name)
    if (fixture.volume && !shapeIsUsable(fixture.volume)) {
      faults.push({
        code: 'volume_shape_invalid',
        subjects: [fixture.name],
        detail: 'This fixture has been given a volume with no usable geometry. A fixture may be '
          + 'named without being measured; it may not be measured wrongly.',
      })
    }
  })

  const channels = new Set<string>()
  cell.parking_pose?.forEach((entry, index) => {
    if (typeof entry.channel !== 'string' || entry.channel.trim() === '') {
      faults.push({
        code: 'parking_channel_unnamed',
        subjects: [`parking_pose[${index}]`],
        detail: 'A parking pose is a command, and a command names its channel.',
      })
      return
    }
    if (channels.has(entry.channel)) {
      faults.push({
        code: 'duplicate_parking_channel',
        subjects: [entry.channel],
        detail: 'One channel, two parking positions. Which one the robot drives to would depend on '
          + 'the order a reader happened to walk the list.',
      })
      return
    }
    channels.add(entry.channel)
  })

  faults.push(...robotFaults(cell))

  return faults
}

/**
 * §87.1's five faults, or none.
 *
 * Split out rather than inlined because all five are conditional on the same thing — **two or more
 * robots** — and a reader has to be able to see that condition once. One robot is §55.2's cell and
 * has no pair to be disjoint from; the placement rule would otherwise refuse every cell anybody has
 * ever written, since a single robot's frame *is* the cell's.
 *
 * Reported in declaration order, then the pairs in index order, so every edition names the same
 * fault first. The disjointness walk skips a robot that already has a fault of its own, for
 * `validateCell`'s duplicate-fixture reason: an operator told that two robots share a volume, when
 * one of them is the unplaced robot they have just been told about, would be reading one problem
 * twice.
 */
function robotFaults(cell: CellDescriptor): CellFault[] {
  const robots = cell.robots ?? []
  const faults: CellFault[] = []
  const slugs = new Set<string>()
  const comparable: Array<{ slug: string; volumes: Array<{ frame: string; shape: Shape }> }> = []

  robots.forEach((robot, index) => {
    const slug = robotSlug(robot)
    if (slug.trim() === '') {
      faults.push({
        code: 'robot_unnamed',
        subjects: [`robots[${index}]`],
        detail: 'A robot a precondition cannot name is a robot no skill can ask about. `env.robot` '
          + 'is its slug.',
      })
      return
    }
    if (slugs.has(slug)) {
      faults.push({
        code: 'duplicate_robot',
        subjects: [slug],
        detail: 'Two entries with one slug: a precondition asking about it would be asking about '
          + 'whichever the reader happened to reach.',
      })
      return
    }
    slugs.add(slug)

    // Everything below is about the pair, so a cell with one robot stops here.
    if (robots.length < 2) return

    if (readPlacement(robot) === null) {
      faults.push({
        code: 'robot_unplaced',
        subjects: [slug],
        detail: 'This cell holds more than one robot, and this one carries no `at`. Its volumes are '
          + 'in its own root frame (§44.2), so nothing can say whether they overlap another '
          + "robot's until somebody says where it stands.",
      })
      return
    }
    const volumes = placedVolumes(robot)
    if (volumes.length === 0) {
      faults.push({
        code: 'robot_unbounded',
        subjects: [slug],
        detail: 'This cell holds more than one robot, and this one draws no volume the workspace '
          + 'guard can evaluate. A robot that may be anywhere is not disjoint from anything.',
      })
      return
    }
    comparable.push({ slug, volumes })
  })

  for (let a = 0; a < comparable.length; a += 1) {
    for (let b = a + 1; b < comparable.length; b += 1) {
      const first = comparable[a]
      const second = comparable[b]
      for (const one of first.volumes) {
        for (const other of second.volumes) {
          if (!intersects(one.shape, other.shape)) continue
          faults.push({
            code: 'shared_volume',
            subjects: [first.slug, second.slug, one.frame, other.frame],
            detail: `${first.slug}'s ${one.frame} and ${second.slug}'s ${other.frame} share a `
              + 'point once both robots are placed. §87.1 gives each robot its own kernel, so '
              + 'nothing on this machine evaluates the pair — the overlap is a claim the platform '
              + 'cannot enforce rather than a hazard it has measured.',
          })
        }
      }
    }
  }

  return faults
}

/**
 * The `env.*` branch of a precondition context (§54.2).
 *
 * Two derived facts and then the operator's own. Derived first and unshadowable, which
 * [`validateCell`] enforces rather than this function silently arbitrating: a cell whose facts
 * overwrote its own name would answer a precondition truthfully about the wrong thing.
 *
 * A cell with faults still produces a context. Refusing to would mean an operator's typo in a
 * fixture name took away the battery check as well, and §54.2 already has an answer for a fact that
 * cannot be read: the precondition naming it is refused, individually, with the path named.
 */
export function cellEnvironment(
  cell: CellDescriptor,
  /**
   * Which robot this context is being assembled for (§87.1).
   *
   * Absent on a single-robot cell, where §55.2's document says nothing about a robot and a slug
   * invented here would be a fact the operator never wrote. Present, and not one the cell names, is
   * reported as the empty string rather than as the slug that was asked for: `env.robot` answers
   * *"which robot does this cell say I am"*, and a robot the cell has not been told about is not one
   * of its robots however sure the caller is.
   */
  slug?: string | null,
): Record<string, unknown> {
  const environment: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(cell.facts ?? {})) {
    if (usableKey(key) && !(DERIVED_FACTS as readonly string[]).includes(key)) {
      environment[key] = value
    }
  }
  environment.cell = cellName(cell)
  environment.fixtures = (cell.fixtures ?? [])
    .map(fixture => fixture.name)
    .filter(name => typeof name === 'string' && name.trim() !== '')
  const declared = cellRobotSlugs(cell)
  environment.robot = typeof slug === 'string' && declared.includes(slug) ? slug : ''
  environment.robots = declared
  return environment
}

/**
 * Every slug the cell names, sorted and without the ones it got wrong.
 *
 * Sorted for `validateCell`'s reason, one field along: the Rust edition holds the list in the order
 * the document had it and a precondition asks `includes`, so order is not semantic — but two
 * editions emitting one list in two orders is a corpus failure, and sorting is the order every
 * edition can produce. A duplicate appears once, because `env.robots` answers *which robots*, and
 * `duplicate_robot` is where the document's mistake is reported.
 */
export function cellRobotSlugs(cell: CellDescriptor): string[] {
  const slugs = new Set<string>()
  for (const robot of cell.robots ?? []) {
    const slug = robotSlug(robot)
    if (slug.trim() !== '') slugs.add(slug)
  }
  return [...slugs].sort()
}

/**
 * The cell's contribution to the `workspace` guard, or null when it draws no volumes.
 *
 * One layer of §38.2's stack, named after the cell so an operator reading a workspace trip can tell
 * which layer stopped the robot. Layers compose by conjunction — a frame must be inside at least
 * one volume of *every* layer that names it — so adding this one can only narrow what the site
 * above it allowed, which is §38.2's rule holding by construction rather than by a check.
 *
 * **A cell that draws nothing adds no layer**, and that is not the same as a cell that allows
 * nothing. An empty layer would forbid every frame it named, and a site that has not yet measured
 * its cell would find the robot refusing to move with no volume anywhere to explain why.
 */
export function cellWorkspaceLayer(cell: CellDescriptor): WorkspaceLayer | null {
  const volumes = (cell.allowed ?? []).filter(volume => shapeIsUsable(volume)
    && typeof volume.frame === 'string' && volume.frame.trim() !== '')
  if (volumes.length === 0) return null
  return { name: `cell:${cellName(cell)}`, volumes }
}

/**
 * The layer one robot of a shared cell contributes, or null (§87.1).
 *
 * A **second** layer rather than a replacement for [`cellWorkspaceLayer`], because §38.2's layers
 * compose by conjunction and two layers is how a cell-wide volume and a per-robot one both hold. A
 * caller that substituted this for the cell's own would be letting a robot's own volume *widen* what
 * the cell allowed everything, which is the one thing a layer may not do.
 *
 * Null for a robot the cell does not name, and null for one that draws nothing — `cellWorkspaceLayer`'s
 * rule, and for the same reason: an empty layer forbids every frame it names, so a robot whose
 * volumes an operator has not drawn yet would refuse to move with no volume anywhere to explain why.
 *
 * The volumes are **unplaced**, in the robot's own root frame, because that is the frame the
 * `workspace` guard resolves poses in (§44.2). `at` exists for the disjointness test in
 * `validateCell` and for nothing else; a guard given cell-frame volumes would be comparing two
 * different coordinate systems and reporting the difference as a margin.
 */
export function cellRobotLayer(cell: CellDescriptor, slug: string): WorkspaceLayer | null {
  const robot = (cell.robots ?? []).find(entry => robotSlug(entry) === slug && slug.trim() !== '')
  if (!robot) return null
  const volumes = (robot.allowed ?? []).filter(volume => shapeIsUsable(volume)
    && typeof volume.frame === 'string' && volume.frame.trim() !== '')
  if (volumes.length === 0) return null
  return { name: `cell:${cellName(cell)}:${slug}`, volumes }
}
