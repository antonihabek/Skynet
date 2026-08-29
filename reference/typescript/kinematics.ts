/**
 * Forward kinematics, the arithmetic underneath it, and the geometry the two pose guards use.
 *
 * The second implementation of `crates/skynet-kernel/src/real.rs` and
 * `crates/skynet-kernel/src/kinematics.rs`, and — like `blackbox.ts` — not a copy of a Hub module,
 * because the Hub does not compute forward kinematics. It exists so that the corpus has two
 * independent implementations to hold to each other, which is the only thing that makes a golden
 * vector worth freezing.
 *
 * # Why the trigonometry is written out
 *
 * `Math.sin` is **not** required to be correctly rounded. ECMAScript permits an implementation-
 * defined approximation, every C library rounds its own way, and a Cortex-M without double-
 * precision hardware runs a third routine again. §31.4 requires two editions to emit byte-identical
 * canonical JSON, so a pose computed through `Math.sin` here and through a libm there would diverge
 * on the first revolute joint — and the divergence would look like a specification disagreement
 * rather than what it is.
 *
 * So the specification defines its own. Addition, subtraction, multiplication and division *are*
 * correctly rounded everywhere, so the same sequence of them produces the same bits on every
 * conforming platform. What follows is that sequence, matching `real.rs` operation for operation.
 * Where a line looks redundant — an unconditional second Cody–Waite step, a fixed five Newton
 * iterations — it is because a branch or an early exit would make the number of operations depend
 * on the input, and an edition could then take a different number of them without noticing.
 *
 * The same argument applies with more force to §7.4's certified tier, where this kernel is mirrored
 * on a separate safety processor: a redundant channel that disagrees with the primary about a clamp
 * is precisely the failure the redundancy exists to detect, and a libm difference would make it
 * fire on correct behaviour.
 *
 * Nothing here uses a fused multiply-add, and JavaScript has none to offer, which is one of the
 * reasons this arithmetic is expressible in both languages at all.
 */

// ── Square root ──────────────────────────────────────────────────────────────

const SQRT_BITS = new DataView(new ArrayBuffer(8))

/**
 * Square root: Newton's iteration from a bit-pattern seed, a fixed five times.
 *
 * `Math.sqrt` is one of the few operations ECMAScript does require to be correctly rounded, so it
 * would agree with a conforming libm — but `core` has no square root at all, the Rust edition
 * therefore computes one, and an oracle that took a shortcut the edition cannot take would be
 * checking the wrong function.
 */
export function sqrt(x: number): number {
  if (Number.isNaN(x) || x < 0) return NaN
  if (x === 0) return x
  if (x === Infinity) return x

  SQRT_BITS.setFloat64(0, x)
  const bits = SQRT_BITS.getBigUint64(0)
  SQRT_BITS.setBigUint64(0, (bits >> 1n) + (1023n << 51n))
  let y = SQRT_BITS.getFloat64(0)
  if (y === 0 || !Number.isFinite(y)) y = x

  for (let step = 0; step < 5; step += 1) {
    y = 0.5 * (y + x / y)
  }
  return y
}

// ── Range reduction ──────────────────────────────────────────────────────────

/** 2/π, to the nearest double: `0x1.45f306dc9c883p-1`. */
const TWO_OVER_PI = 6.36619772367581382433e-1
/** π/2 with its low 33 bits cleared, so `k * PIO2_1` is exact for every admitted `k`. */
const PIO2_1 = 1.57079632673412561417e0
/** π/2 − PIO2_1, split so the second step recovers the bits the first dropped. */
const PIO2_2 = 6.0771005063039659766e-11
/** The remainder of that split. */
const PIO2_2T = 2.02226624879595063154e-21

/**
 * The largest quadrant count the reduction is exact for.
 *
 * PIO2_1 carries 33 significant bits, so `k * PIO2_1` is exact while `k` fits in 20 — about
 * 1.65 × 10⁶ radians, a quarter of a million revolutions. Past that the caller is told rather than
 * handed a pose computed from a reduction that has lost its low bits.
 */
const MAX_QUADRANTS = 1048576

/** Round half away from zero. ECMAScript's `Math.round` sends −0.5 to −0; this does not. */
function roundHalfAway(v: number): number {
  const shifted = v >= 0 ? v + 0.5 : v - 0.5
  return Math.trunc(shifted)
}

interface Reduced {
  r: number
  quadrant: number
  exact: boolean
}

function reduce(x: number): Reduced {
  const k = roundHalfAway(x * TWO_OVER_PI)
  if (Number.isNaN(k) || Math.abs(k) >= MAX_QUADRANTS) {
    return { r: 0, quadrant: 0, exact: false }
  }

  const t = x - k * PIO2_1
  const w0 = k * PIO2_2
  const r0 = t - w0
  const w1 = k * PIO2_2T - (t - r0 - w0)
  const r = r0 - w1

  return { r, quadrant: ((k % 4) + 4) % 4, exact: true }
}

// ── The kernel polynomials, on [-π/4, π/4] ───────────────────────────────────
//
// fdlibm's minimax coefficients, with fdlibm's own digits, so that a reviewer can diff them against
// the published table without converting anything. The Rust edition writes the same characters.

const S1 = -1.66666666666666324348e-1
const S2 = 8.33333333332248946124e-3
const S3 = -1.98412698298579493134e-4
const S4 = 2.75573137070700676789e-6
const S5 = -2.50507602534068634195e-8
const S6 = 1.58969099521155010221e-10

const C1 = 4.16666666666666019037e-2
const C2 = -1.38888888888741095749e-3
const C3 = 2.48015872894767294178e-5
const C4 = -2.75573143513906633035e-7
const C5 = 2.0875723212981748279e-9
const C6 = -1.13596475577881948265e-11

function kernelSin(x: number): number {
  const z = x * x
  const poly = S1 + z * (S2 + z * (S3 + z * (S4 + z * (S5 + z * S6))))
  return x + x * z * poly
}

function kernelCos(x: number): number {
  const z = x * x
  const poly = C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6))))
  return 1 - 0.5 * z + z * z * poly
}

/**
 * Sine.
 *
 * NaN for a non-finite angle and for one outside the range the reduction is exact over. The NaN
 * propagates into the pose, and `solve` reads a non-finite pose as an unresolved frame and names it
 * — §7.2's rule applied to arithmetic: a guard that cannot compute its input reports rather than
 * acting on a number it does not trust.
 */
export function sin(x: number): number {
  if (!Number.isFinite(x)) return NaN
  const reduced = reduce(x)
  if (!reduced.exact) return NaN
  switch (reduced.quadrant) {
    case 0: return kernelSin(reduced.r)
    case 1: return kernelCos(reduced.r)
    case 2: return -kernelSin(reduced.r)
    default: return -kernelCos(reduced.r)
  }
}

/** Cosine. Same domain rule as `sin`. */
export function cos(x: number): number {
  if (!Number.isFinite(x)) return NaN
  const reduced = reduce(x)
  if (!reduced.exact) return NaN
  switch (reduced.quadrant) {
    case 0: return kernelCos(reduced.r)
    case 1: return -kernelSin(reduced.r)
    case 2: return -kernelCos(reduced.r)
    default: return kernelSin(reduced.r)
  }
}

// ── Transforms ───────────────────────────────────────────────────────────────

export type Vec3 = [number, number, number]
export type Quat = [number, number, number, number]

/** A rigid transform: metres, and a quaternion ordered `xyzw` (§44.4). */
export interface Transform {
  translation: Vec3
  rotation: Quat
}

export const IDENTITY: Transform = { translation: [0, 0, 0], rotation: [0, 0, 0, 1] }

/** Hamilton product, `xyzw` in and `xyzw` out. */
export function quaternionProduct(a: Quat, b: Quat): Quat {
  const [ax, ay, az, aw] = a
  const [bx, by, bz, bw] = b
  return [
    aw * bx + ax * bw + ay * bz - az * by,
    aw * by - ax * bz + ay * bw + az * bx,
    aw * bz + ax * by - ay * bx + az * bw,
    aw * bw - ax * bx - ay * by - az * bz,
  ]
}

/** Rotate a point by a quaternion. Active rotation, per §44.4. */
export function rotate(q: Quat, v: Vec3): Vec3 {
  const [qx, qy, qz, qw] = q
  const tx = 2 * (qy * v[2] - qz * v[1])
  const ty = 2 * (qz * v[0] - qx * v[2])
  const tz = 2 * (qx * v[1] - qy * v[0])
  return [
    v[0] + qw * tx + (qy * tz - qz * ty),
    v[1] + qw * ty + (qz * tx - qx * tz),
    v[2] + qw * tz + (qx * ty - qy * tx),
  ]
}

/** `parent` followed by `local`: the child's transform in the parent's parent. */
export function compose(parent: Transform, local: Transform): Transform {
  const rotated = rotate(parent.rotation, local.translation)
  return {
    translation: [
      parent.translation[0] + rotated[0],
      parent.translation[1] + rotated[1],
      parent.translation[2] + rotated[2],
    ],
    rotation: quaternionProduct(parent.rotation, local.rotation),
  }
}

function isFiniteTransform(t: Transform): boolean {
  return t.translation.every(Number.isFinite) && t.rotation.every(Number.isFinite)
}

// ── The flattened frame set ──────────────────────────────────────────────────

export type JointKind = 'revolute' | 'continuous' | 'prismatic' | 'ball' | 'floating'

const SCALAR_JOINTS: JointKind[] = ['revolute', 'continuous', 'prismatic']

export interface Mimic {
  joint: string
  multiplier: number
  offset: number
}

export interface Joint {
  name: string
  kind: JointKind
  axis: Vec3
  mimic?: Mimic
}

/** A sphere-swept segment: every point within `radius_m` of the segment from `a` to `b`. */
export interface Capsule {
  a: Vec3
  b: Vec3
  radius_m: number
}

/** One frame, flattened: the parent is an index strictly smaller than this frame's own. */
export interface Frame {
  name: string
  parent: number | null
  local: Transform
  joint?: Joint
  collision?: Capsule
  /** Frames this one may touch, by index. */
  ignore: number[]
}

export type FrameSetFault =
  | 'empty'
  | 'too_large'
  | 'root_not_first'
  | 'not_topological'
  | 'parent_out_of_range'
  | 'ignore_out_of_range'

export const MAX_FRAMES = 128
export const MAX_POSE_TRIPS = 8
export const MAX_UNRESOLVED = 8

/**
 * Check the ordering invariant once, so that no cycle ever checks it again.
 *
 * The invariant — frame `i`'s parent index is `< i` — is what makes forward kinematics a single
 * forward pass and makes a cycle in the graph unrepresentable rather than a hang in a control loop.
 */
export function checkFrameSet(frames: Frame[]): FrameSetFault | null {
  if (frames.length === 0) return 'empty'
  if (frames.length > MAX_FRAMES) return 'too_large'
  if (frames[0].parent !== null) return 'root_not_first'
  for (let index = 0; index < frames.length; index += 1) {
    const parent = frames[index].parent
    if (parent === null) {
      if (index > 0) return 'root_not_first'
    } else {
      if (parent >= frames.length) return 'parent_out_of_range'
      if (parent >= index) return 'not_topological'
    }
    for (const ignored of frames[index].ignore) {
      if (ignored >= frames.length) return 'ignore_out_of_range'
    }
  }
  return null
}

/**
 * Whether a frame's pose is computable at all, whatever the joint values are.
 *
 * False when the walk to the root passes through a joint one scalar cannot describe. A static
 * property, and knowing it statically turns "the guard holds the robot every cycle" into "the guard
 * reported itself unavailable when the lease was granted, naming the frame" (§44.5).
 */
export function staticallyResolvable(frames: Frame[], index: number): boolean {
  let here = index
  for (;;) {
    const frame = frames[here]
    if (frame === undefined) return false
    if (frame.joint && !SCALAR_JOINTS.includes(frame.joint.kind)) return false
    if (frame.parent === null) return true
    here = frame.parent
  }
}

/** How many frames carry collision geometry this kernel can both read and place. */
export function collisionBodies(frames: Frame[]): number {
  return frames.filter(
    (frame, index) => frame.collision !== undefined && staticallyResolvable(frames, index),
  ).length
}

/** How many pairs the self-collision guard tests each cycle. */
export function collisionPairs(frames: Frame[]): number {
  const n = collisionBodies(frames)
  return (n * (n - 1)) / 2
}

// ── Solving ──────────────────────────────────────────────────────────────────

/** How a joint is driven this cycle, and where it is now. */
export interface JointBinding {
  joint: string
  /** The command channel driving it, when the transport plan bound one to a **position**. */
  channel: string | null
  measured: number
}

export interface Solved {
  poses: Array<Transform | null>
  unresolved: number[]
  /** Whether every joint value came from this cycle's command. */
  lookahead: boolean
}

/**
 * Compute every frame's pose in the root frame: one forward pass.
 *
 * A frame is **unresolved** when its parent is, when its joint has no value, when its joint kind
 * needs more than a scalar, or when the arithmetic produced something that is not finite — and
 * unresolved propagates down the chain, because a pose relative to an unknown pose is unknown.
 */
export function solve(
  frames: Frame[],
  bindings: JointBinding[],
  command: Record<string, number>,
): Solved {
  const poses: Array<Transform | null> = []
  const unresolved: number[] = []
  let lookahead = true

  const valueOf = (joint: string): number | null => {
    const binding = bindings.find(entry => entry.joint === joint)
    if (binding === undefined) return null
    if (binding.channel === null) {
      lookahead = false
      return binding.measured
    }
    const value = command[binding.channel]
    if (value === undefined) {
      lookahead = false
      return binding.measured
    }
    return value
  }

  for (let index = 0; index < frames.length; index += 1) {
    const frame = frames[index]
    let parentPose: Transform = IDENTITY
    if (frame.parent !== null) {
      const resolved = poses[frame.parent]
      if (!resolved) {
        poses.push(null)
        unresolved.push(index)
        continue
      }
      parentPose = resolved
    }

    let local = frame.local
    if (frame.joint) {
      // A mimic is followed exactly one step. URDF permits no more, and a chain of them would need
      // a traversal with its own cycle check inside the control loop.
      const driver = frame.joint.mimic
      const raw = driver ? valueOf(driver.joint) : valueOf(frame.joint.name)
      const value = raw === null || !driver ? raw : driver.multiplier * raw + driver.offset
      if (value === null) {
        poses.push(null)
        unresolved.push(index)
        continue
      }
      const moved = motion(frame.joint, value)
      if (moved === null) {
        poses.push(null)
        unresolved.push(index)
        continue
      }
      local = compose(local, moved)
    }

    const pose = compose(parentPose, local)
    if (!isFiniteTransform(pose)) {
      poses.push(null)
      unresolved.push(index)
      continue
    }
    poses.push(pose)
  }

  return { poses, unresolved, lookahead }
}

/**
 * The transform a joint value produces, in the joint's own frame.
 *
 * Null where a scalar cannot describe the motion, or where the axis is degenerate. A zero-length
 * axis is a body-description defect and not a fixed joint: reading it as one would silently weld a
 * limb in the guard's model of the robot while it moved in the world.
 */
function motion(joint: Joint, value: number): Transform | null {
  if (!Number.isFinite(value)) return null
  if (!SCALAR_JOINTS.includes(joint.kind)) return null
  const axis = unit(joint.axis)
  if (axis === null) return null

  if (joint.kind === 'prismatic') {
    return {
      translation: [axis[0] * value, axis[1] * value, axis[2] * value],
      rotation: [0, 0, 0, 1],
    }
  }
  const half = 0.5 * value
  const s = sin(half)
  const c = cos(half)
  if (!Number.isFinite(s) || !Number.isFinite(c)) return null
  return { translation: [0, 0, 0], rotation: [axis[0] * s, axis[1] * s, axis[2] * s, c] }
}

function unit(v: Vec3): Vec3 | null {
  const squared = v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
  if (!Number.isFinite(squared) || squared <= 0) return null
  const length = sqrt(squared)
  if (!Number.isFinite(length) || length <= 0) return null
  return [v[0] / length, v[1] / length, v[2] / length]
}

// ── Volumes ──────────────────────────────────────────────────────────────────

export type Shape =
  | { kind: 'box'; min: Vec3; max: Vec3 }
  | { kind: 'sphere'; centre: Vec3; radius_m: number }

export interface Volume {
  frame: string
  shape: Shape
}

/**
 * A layer of the operator's workspace configuration (§38.2).
 *
 * Layers compose by **conjunction**: a frame must be inside at least one volume of every layer that
 * names it. That is "each layer may only narrow" as a predicate rather than as geometry —
 * intersecting a box with a sphere is a shape neither of them is, and a composition rule producing
 * a third shape would be one no operator could check by reading their own configuration.
 */
export interface WorkspaceLayer {
  name: string
  volumes: Volume[]
}

/** Signed distance from a point to a volume's boundary: **positive inside**. */
export function margin(shape: Shape, point: Vec3): number {
  if (shape.kind === 'sphere') {
    const dx = point[0] - shape.centre[0]
    const dy = point[1] - shape.centre[1]
    const dz = point[2] - shape.centre[2]
    return shape.radius_m - sqrt(dx * dx + dy * dy + dz * dz)
  }

  const outside: Vec3 = [0, 0, 0]
  let anyOutside = false
  let insideMargin = Infinity
  for (let axis = 0; axis < 3; axis += 1) {
    const below = shape.min[axis] - point[axis]
    const above = point[axis] - shape.max[axis]
    if (below > 0) {
      outside[axis] = below
      anyOutside = true
    } else if (above > 0) {
      outside[axis] = above
      anyOutside = true
    }
    const toLow = point[axis] - shape.min[axis]
    const toHigh = shape.max[axis] - point[axis]
    const nearest = toLow < toHigh ? toLow : toHigh
    if (nearest < insideMargin) insideMargin = nearest
  }
  if (anyOutside) {
    return -sqrt(outside[0] * outside[0] + outside[1] * outside[1] + outside[2] * outside[2])
  }
  return insideMargin
}

// ── Distance between two capsules ────────────────────────────────────────────

function sub(a: Vec3, b: Vec3): Vec3 {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

function dot(a: Vec3, b: Vec3): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

function clamp01(v: number): number {
  if (v > 1) return 1
  if (v > 0) return v
  return 0
}

/**
 * Closest distance between two segments, squared.
 *
 * Squared, because the self-collision guard's **decision** compares it against a squared threshold
 * and no square root can therefore move a verdict. The root is taken only for the number the report
 * carries.
 */
export function segmentDistanceSquared(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3): number {
  const d1 = sub(q1, p1)
  const d2 = sub(q2, p2)
  const r = sub(p1, p2)
  const a = dot(d1, d1)
  const e = dot(d2, d2)
  const f = dot(d2, r)

  const epsilon = 1e-18
  let s: number
  let t: number
  if (a <= epsilon && e <= epsilon) {
    return dot(r, r)
  }
  if (a <= epsilon) {
    s = 0
    t = clamp01(f / e)
  } else {
    const c = dot(d1, r)
    if (e <= epsilon) {
      t = 0
      s = clamp01(-c / a)
    } else {
      const b = dot(d1, d2)
      const denominator = a * e - b * b
      s = denominator > epsilon ? clamp01((b * f - c * e) / denominator) : 0
      t = (b * s + f) / e
      if (t < 0) {
        t = 0
        s = clamp01(-c / a)
      } else if (t > 1) {
        t = 1
        s = clamp01((b - c) / a)
      }
    }
  }

  const c1: Vec3 = [p1[0] + d1[0] * s, p1[1] + d1[1] * s, p1[2] + d1[2] * s]
  const c2: Vec3 = [p2[0] + d2[0] * t, p2[1] + d2[1] * t, p2[2] + d2[2] * t]
  const diff = sub(c1, c2)
  return dot(diff, diff)
}

/** A capsule's two endpoints, in the root frame. */
export function capsuleInWorld(pose: Transform, capsule: Capsule): [Vec3, Vec3] {
  const a = rotate(pose.rotation, capsule.a)
  const b = rotate(pose.rotation, capsule.b)
  return [
    [pose.translation[0] + a[0], pose.translation[1] + a[1], pose.translation[2] + a[2]],
    [pose.translation[0] + b[0], pose.translation[1] + b[1], pose.translation[2] + b[2]],
  ]
}

/** Whether `a` lists `b`, or `b` lists `a`. Symmetric on purpose. */
export function ignores(frames: Frame[], a: number, b: number): boolean {
  return frames[a].ignore.includes(b) || frames[b].ignore.includes(a)
}

/** Whether one frame is the other's parent. Excluded from the collision test unconditionally. */
export function adjacent(frames: Frame[], a: number, b: number): boolean {
  return frames[a].parent === b || frames[b].parent === a
}

/** The specified square root, for a consumer reporting a distance. */
export function distance(squared: number): number {
  return sqrt(Math.abs(squared))
}

// ── Flattening a descriptor's frame graph ────────────────────────────────────

import { validateFrames, type FrameFault, type FrameGraph } from './embodiment.ts'

function fault(code: FrameFault['code'], subjects: string[], detail: string): FrameFault {
  return { code, subjects, detail }
}

/**
 * Flatten a §44.3 frame graph into the order the kernel reads.
 *
 * Breadth-first from the root, so a parent is always emitted before its children and the ordering
 * invariant holds by construction rather than by a sort that has to be checked afterwards. The
 * output order is a function of the tree and of the descriptor's own node order among siblings,
 * which makes it stable across editions — a requirement, because verdicts name frames and the
 * corpus compares those names in order.
 */
export function flatten(graph: FrameGraph): { frames: Frame[] } | { fault: FrameFault } {
  const invalid = validateFrames(graph)
  if (invalid) return { fault: invalid }
  if (graph.nodes.length > MAX_FRAMES) {
    return {
      fault: fault(
        'too_large',
        [],
        `The body declares ${graph.nodes.length} frames and this kernel is built for ` +
          `${MAX_FRAMES}. Half a frame graph is a robot with a limb missing from its own ` +
          `collision model, so the graph is refused rather than truncated.`,
      ),
    }
  }

  const rootIndex = graph.nodes.findIndex(node => node.name === graph.root)
  if (rootIndex < 0) {
    return {
      fault: fault('root_missing', [graph.root], `The declared root ${graph.root} is not one of the frames.`),
    }
  }

  const order: number[] = [rootIndex]
  for (let cursor = 0; cursor < order.length; cursor += 1) {
    const parent = graph.nodes[order[cursor]]
    graph.nodes.forEach((node, index) => {
      if (node.parent === parent.name) order.push(index)
    })
  }

  if (order.length !== graph.nodes.length) {
    const orphan = graph.nodes.find((_, index) => !order.includes(index))?.name ?? ''
    return {
      fault: fault(
        'unreachable',
        [orphan],
        `Frame ${orphan} is not reachable from the root ${graph.root}. Its parent chain is valid ` +
          `and leads somewhere else, which is a second tree in one document.`,
      ),
    }
  }

  const position = new Array<number>(graph.nodes.length).fill(0)
  order.forEach((index, slot) => {
    position[index] = slot
  })

  const frames: Frame[] = []
  for (const index of order) {
    const node = graph.nodes[index]
    const parent =
      node.parent === null
        ? null
        : position[graph.nodes.findIndex(other => other.name === node.parent)]

    let joint: Joint | undefined
    if (node.joint !== undefined && node.joint !== null) {
      if (node.joint_type === undefined || node.joint_type === null) {
        return {
          fault: fault(
            'unknown_joint_type',
            [node.name],
            `Frame ${node.name} names a joint and no joint type, so nothing can tell whether it ` +
              `turns or slides.`,
          ),
        }
      }
      if (!SCALAR_JOINTS.includes(node.joint_type) && node.joint_type !== 'ball' && node.joint_type !== 'floating') {
        return {
          fault: fault(
            'unknown_joint_type',
            [node.name],
            `Frame ${node.name} names joint type ${node.joint_type}, which is not one of the five ` +
              `§44.3 admits. Reading it as revolute is how a prismatic axis becomes a rotation.`,
          ),
        }
      }
      joint = {
        name: node.joint,
        kind: node.joint_type,
        // URDF's default, stated: the axis is `1 0 0` when the file is silent.
        axis: node.axis ?? [1, 0, 0],
        ...(node.mimics
          ? {
              mimic: {
                joint: node.mimics,
                multiplier: node.mimic_multiplier ?? 1,
                offset: node.mimic_offset ?? 0,
              },
            }
          : {}),
      }
    }

    const ignore: number[] = []
    for (const name of node.collision_ignore ?? []) {
      const found = graph.nodes.findIndex(other => other.name === name)
      if (found >= 0) ignore.push(position[found])
    }
    ignore.sort((a, b) => a - b)

    frames.push({
      name: node.name,
      parent,
      local: {
        translation: node.translation ?? [0, 0, 0],
        // Absent means no rotation, and the identity quaternion is `xyzw = 0 0 0 1` — not
        // `0 0 0 0`, which is not a rotation at all and would collapse every pose below it.
        rotation: node.rotation ?? [0, 0, 0, 1],
      },
      ...(joint ? { joint } : {}),
      ...(node.collision_shape
        ? {
            collision: {
              a: node.collision_shape.a,
              b: node.collision_shape.b,
              radius_m: node.collision_shape.radius_m,
            },
          }
        : {}),
      ignore,
    })
  }

  return { frames }
}
