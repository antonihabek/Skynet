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
 */

import type { Volume, WorkspaceLayer } from './kinematics.ts'

/** A thing in the cell that is not the robot. */
export interface Fixture {
  name: string
  /** Where it is, when the operator has drawn it. Fixtures may be named without being measured. */
  volume?: Volume | null
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
}

/**
 * Facts this file derives, which a cell's own `facts` may not shadow.
 *
 * A precondition reading `env.cell` must be reading the cell's name, on every robot, whatever
 * somebody typed into their configuration.
 */
export const DERIVED_FACTS = ['cell', 'fixtures'] as const

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
export function cellEnvironment(cell: CellDescriptor): Record<string, unknown> {
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
  return environment
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
