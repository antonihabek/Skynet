/**
 * The Embodiment Descriptor and Embodiment Transport.
 *
 * This is the module the whole operating system is built around, so it is worth being precise
 * about the problem it solves.
 *
 * A phone app does not care whether the screen is 6.1" or 6.7". A robot policy cares enormously
 * whether the arm has six joints or seven, whether joint 3 is rated for 80 Nm or 200 Nm, and
 * whether the thing it calls `left_shoulder_pitch` is the joint this robot calls `J2`. Android
 * could get away with an abstraction that hides the hardware. A robot OS cannot: **the body is
 * part of the API**, and pretending otherwise is how you destroy a wrist.
 *
 * The industry's current answer is that a policy is trained against an implicit body and is then
 * hand-ported to every other body by an engineer who reads two URDFs side by side. That does not
 * scale to a catalogue of thousands of policies and hundreds of robots, and it is the single
 * reason a model published for one humanoid is useless on the humanoid next to it.
 *
 * ── What this module does ───────────────────────────────────────────────────
 *
 * 1. Makes the body **explicit and typed** — an `EmbodimentDescriptor` published and signed by
 *    the vendor, digested like any other Skynet artifact.
 * 2. Gives joints a **canonical role vocabulary** so two vendors' naming schemes can be related
 *    mechanically instead of by hand. This is the USB-descriptor move: the wire names stay
 *    vendor-specific, the semantics become shared.
 * 3. Resolves a **transport plan** — for every channel of the policy's action space, what this
 *    body will do with it: drive it directly, remap it to a differently-named joint, retarget it
 *    through task space, synthesise it, drop it, or refuse.
 *
 * ── Why the plan is pure, and computed identically in two places ────────────
 *
 * The same function runs on the Hub (so a repository page can answer "does this run on my G1?"
 * before anybody downloads forty gigabytes) and inside the OS on the robot (which must reach the
 * same answer with no network at all). A planner that lived only on the server would make every
 * robot's autonomy depend on connectivity; one that lived only on the robot would make the
 * catalogue unable to answer the question that decides whether to download. So it lives here,
 * pure, with no I/O, and both sides import it.
 *
 * ── What it deliberately does not do ────────────────────────────────────────
 *
 * It does not *perform* the retargeting. Computing inverse kinematics is the runtime's job, in
 * the control loop, in a real-time context. This module decides **what is possible and under
 * what ceilings**, and hands the ceilings to the Safety Kernel, which is the only component that
 * can enforce them. Deciding and enforcing are separated on purpose: the decision is complex and
 * runs once; the enforcement is trivial and runs at 1 kHz.
 *
 * It also never invents a number. A body with no declared envelope produces a plan that says so,
 * exactly as `skynet_robots.limits` distinguishes "nobody has declared limits" from "limits were
 * declared and are empty". A transport plan that quietly assumed a plausible torque rating would
 * be a safety check that lies, which is the worst artifact this system could produce.
 */

import type { SafetyEnvelope } from './types.ts'

// ── Canonical role vocabulary ────────────────────────────────────────────────

/**
 * Kinematic chains a role may belong to.
 *
 * Deliberately small and body-plan agnostic. `aux` exists so a body with a structure nobody
 * anticipated — a snake robot's segments, a second mast, a tail-mounted sensor boom — can still
 * declare roles that parse, rather than being pushed into a wrong-but-close chain because the
 * vocabulary had no honest slot for it.
 */
export const CANONICAL_CHAINS = [
  'left_arm', 'right_arm', 'left_leg', 'right_leg',
  'left_hand', 'right_hand', 'torso', 'neck', 'head', 'base', 'tail', 'aux',
] as const
export type CanonicalChain = (typeof CANONICAL_CHAINS)[number]

/**
 * Axis of motion, when a link has more than one degree of freedom.
 *
 * A three-DoF shoulder is three joints in every real driver, and the axis suffix is what keeps
 * `left_arm.shoulder.pitch` from colliding with `left_arm.shoulder.roll` while still letting
 * both be recognised as the same shoulder on a different robot.
 */
export const CANONICAL_AXES = [
  'pitch', 'yaw', 'roll', 'slide', 'flex', 'abduct', 'rotate', 'grip',
] as const
export type CanonicalAxis = (typeof CANONICAL_AXES)[number]

export interface ParsedRole {
  chain: CanonicalChain
  link: string
  axis: CanonicalAxis | null
  /** Ordinal for repeated links — `base.wheel#2`, `right_hand.finger#1`. */
  index: number | null
}

const ROLE_PATTERN = /^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#(\d+))?$/

/**
 * Parse a canonical role.
 *
 * Returns null rather than throwing, and the caller treats null as "this joint can only be
 * matched by exact name". That is the extensibility rule that keeps the vocabulary honest: an
 * unrecognised role never *silently* maps to something plausible. A body the vocabulary does not
 * describe is still usable — it just cannot participate in cross-body remapping, which is the
 * truthful outcome, because nobody has established what its joints mean.
 */
export function parseRole(role: string | null | undefined): ParsedRole | null {
  if (!role) return null
  const match = ROLE_PATTERN.exec(role.trim().toLowerCase())
  if (!match) return null

  const [, chain, link, axis, index] = match
  if (!(CANONICAL_CHAINS as readonly string[]).includes(chain)) return null
  if (axis && !(CANONICAL_AXES as readonly string[]).includes(axis)) return null

  return {
    chain: chain as CanonicalChain,
    link,
    axis: (axis as CanonicalAxis) ?? null,
    index: index ? Number(index) : null,
  }
}

/** Canonical string form, so two parsed roles can be compared as keys. */
export function roleKey(parsed: ParsedRole): string {
  return `${parsed.chain}.${parsed.link}${parsed.axis ? `.${parsed.axis}` : ''}${
    parsed.index === null ? '' : `#${parsed.index}`
  }`
}

// ── The descriptor ───────────────────────────────────────────────────────────

export const JOINT_TYPES = ['revolute', 'prismatic', 'continuous', 'fixed'] as const
export type JointType = (typeof JOINT_TYPES)[number]

export const CONTROL_MODES = ['position', 'velocity', 'torque', 'trajectory', 'cartesian'] as const
export type ControlMode = (typeof CONTROL_MODES)[number]

export const BASE_KINDS = ['fixed', 'wheeled', 'tracked', 'legged', 'aerial', 'rail'] as const
export type BaseKind = (typeof BASE_KINDS)[number]

export interface JointLimits {
  position_min_rad?: number
  position_max_rad?: number
  velocity_max_rad_s?: number
  effort_max_nm?: number
}

export interface EmbodimentJoint {
  /** The name the vendor's driver actually uses on the wire. Never normalised away. */
  name: string
  /** Canonical role, or null when the vendor declared none. */
  role: string | null
  type: JointType
  limits: JointLimits
  /** Modes this specific joint accepts. A wrist may be position-only on a torque-capable robot. */
  control_modes: ControlMode[]
}

export interface EmbodimentEndEffector {
  name: string
  role: string | null
  kind: 'gripper' | 'dexterous_hand' | 'suction' | 'tool' | 'none'
  dof: number
  max_grip_force_n?: number
  max_opening_m?: number
}

export const EMBODIMENT_MODALITIES = [
  'rgb', 'depth', 'stereo', 'point_cloud', 'imu', 'force_torque', 'tactile', 'audio', 'joint_state',
] as const
export type EmbodimentModality = (typeof EMBODIMENT_MODALITIES)[number]

export interface EmbodimentSensor {
  name: string
  role: string | null
  modality: EmbodimentModality
  width?: number
  height?: number
  fps?: number
  /** Frame the readings are expressed in, so extrinsics are resolvable. */
  frame?: string
}

export interface EmbodimentDescriptor {
  schema_version: 1
  robot_slug: string
  /** sha256 over the canonicalised descriptor. Every transport plan pins it. */
  digest: string | null
  /**
   * Whether a signature over `digest` verified against a key registered to the vendor's
   * namespace. False means the body description is unattested — usable, and reported as such,
   * because an unsigned descriptor is still better than the guesswork it replaces.
   */
  verified: boolean
  base: BaseKind
  dof: number
  joints: EmbodimentJoint[]
  end_effectors: EmbodimentEndEffector[]
  sensors: EmbodimentSensor[]
  control: {
    modes: ControlMode[]
    rate_hz_max?: number
    rate_hz_min?: number
    /**
     * Command accepted to torque applied, round trip. This is the number that decides whether a
     * policy trained at 50 Hz is stable on this body, and almost nobody publishes it.
     */
    command_latency_ms?: number
  }
  /** Reuses the Hub's envelope shape. Null means nobody has declared one — not "no limits". */
  envelope: SafetyEnvelope | null
  /** IK solvers resident on this robot. Empty means task-space retargeting is not available. */
  solvers: string[]
  /**
   * Capability tokens this body **and its adapter** provide (§8).
   *
   * The set `admitPackage` checks a package's `embodiment.capabilities` against. It lives here
   * rather than anywhere else for two reasons: this descriptor is produced by the adapter, for the
   * body, so “the robot plus its adapter” is precisely what it is in a position to answer — and
   * §11's invariant is that no decision on a robot requires a round trip, which rules out asking
   * the Hub at install time on a machine that has been offline for a week.
   *
   * **Empty is a claim, not an absence.** A body that declares no capabilities provides none, and a
   * package needing one is refused — the direction that fails safe. Deliberately not nullable:
   * “nobody said what this robot can do” would leave admission choosing between refusing
   * everything and admitting everything, and both are worse than requiring a vendor to write the
   * list down.
   */
  capabilities?: string[]
  /**
   * Vendor extension tokens, beyond the canonical vocabulary.
   *
   * Separate from `capabilities` for the reason §5.1 keeps roles extensible: a token nobody
   * standardised must be expressible without being mistaken for one that was.
   */
  extensions?: string[]
  /**
   * Whether this body is simulated (§35.3, §70).
   *
   * The one field on this descriptor whose absence means `false` rather than "nobody said". A real
   * robot should not have to declare what it is not, and the obligation to declare sits on the
   * simulator driver, where §35.3 puts it. Made nullable it would produce "nobody said whether this
   * robot is real", which is a question a physical machine's existence answers.
   *
   * It caps the certification award at `guest` — see `awardCertification` — and it is read from
   * here rather than from the driver's declared ceiling, because a simulator driver written in C++
   * has no language ceiling to fall back on.
   */
  simulator?: boolean
  /**
   * The frame graph (§44.3), or `null` where the generator could not express this body's tree.
   *
   * Absent, null and empty are three different bodies: absent is a schema that predates the field,
   * null is a generator that said it could not, and an empty `nodes` array is a body with no frames
   * at all — a claim nobody should make. All three leave the pose guards unavailable; only the
   * middle one names a reason.
   */
  frames?: FrameGraph | null
}

// ── The frame graph, §44.3 ───────────────────────────────────────────────────
//
// Pinned here rather than left to whichever tool wrote a file first. Two generators emit it —
// `skynet descriptor from-urdf` and the MuJoCo driver — and the kernel will read it; a second
// edition emitting a different spelling of the same tree is a divergence no test catches, precisely
// because nothing consumes `frames` yet and there is therefore nothing to fail.

/** Where a frame's transform came from. Calibrated wins over nominal, and the frame records which. */
export type FrameSource = 'descriptor' | 'calibration'

export interface FrameNode {
  name: string
  /** Null on the root, and on nothing else. */
  parent: string | null
  kind: 'root' | 'static' | 'dynamic'
  /** Metres, from the parent frame. Absent on the root, which has nothing to be relative to. */
  translation?: [number, number, number]
  /** Ordered `xyzw` (§44.4). Absent on the root. */
  rotation?: [number, number, number, number]
  source?: FrameSource
  /** Present exactly when `kind` is `dynamic`. */
  joint?: string
  joint_type?: 'revolute' | 'continuous' | 'prismatic' | 'ball' | 'floating'
  /** In this frame, before the joint is applied. */
  axis?: [number, number, number]
  /** The joint this one follows, where it is a linkage rather than a degree of freedom. */
  mimics?: string
  /**
   * `value = multiplier * driver + offset`. Absent means one.
   *
   * Present because the commonest linkage in robotics is a parallel gripper whose second finger
   * runs at −1, and a field that could only say "follows" would put that finger on the wrong side
   * of the object — inside the guard whose whole job is knowing where things are.
   */
  mimic_multiplier?: number
  /** Radians or metres, matching the joint kind. Absent means zero. */
  mimic_offset?: number
  collision?: boolean
  /**
   * The geometry, where it is a shape the Safety Kernel can check.
   *
   * `collision: true` with no shape is a real and common body: the vendor's model has meshes and
   * nobody has reduced them to a primitive. It is not the same as `collision: false` and not the
   * same as a shape — the first says "there is geometry here this kernel cannot evaluate", and the
   * `self_collision` guard reports itself unavailable naming those frames rather than testing a
   * body with limbs missing from it.
   */
  collision_shape?: CollisionShape | null
  /**
   * Frames this one may touch.
   *
   * Every real body has some. A humanoid's pelvis and torso overlap by design; URDF cannot say so
   * and SRDF's `disable_collisions` exists for exactly this. Without it the guard trips on the
   * robot standing still, and a guard that does that is one an operator switches off.
   */
  collision_ignore?: string[]
  /** Null where the frame carries no geometry. */
  mass_kg?: number | null
}

/**
 * Collision geometry, as a sphere-swept segment.
 *
 * One primitive, and the right one for a kernel that must finish inside a control cycle: a capsule
 * is a sphere when its endpoints coincide, it bounds a cylinder exactly and a link's real geometry
 * conservatively, and the distance between two of them is a closed form. Conservative is the
 * correct direction of error — the shape contains what it stands for, so the guard trips early
 * rather than late.
 */
export interface CollisionShape {
  /** `capsule` today. A body that is something else has to say so rather than be misread. */
  kind: string
  a: [number, number, number]
  b: [number, number, number]
  radius_m: number
}

/** Shapes the kernel can evaluate. */
export const COLLISION_SHAPES = ['capsule']

export interface FrameGraph {
  /** Names the node whose parent is null. `base_link` by convention (§44.4), not by requirement. */
  root: string
  /** `rep103` today. The field exists so a body that is *not* REP-103 has to say so. */
  convention: string
  /** What produced the tree. Provenance, never a licence to read the other fields differently. */
  generator?: string | null
  nodes: FrameNode[]
}

/**
 * Why a frame graph cannot be used.
 *
 * Returned rather than thrown, and carrying the frames it is about, because §44.5 requires the pose
 * guards to report unavailable **with the missing frame named**. "The frame graph is malformed" is
 * not a sentence an operator can act on.
 */
export interface FrameFault {
  code:
    | 'empty'
    | 'root_missing'
    | 'several_roots'
    | 'duplicate_name'
    | 'parent_missing'
    | 'cycle'
    | 'joint_mismatch'
    | 'unknown_kind'
    | 'unknown_shape'
    | 'collision_mismatch'
    | 'ignore_missing'
    // Raised by `validateFrameBindings`, which needs the joint list `validateFrames` never sees.
    | 'dangling_joint'
    | 'mimic_missing'
    | 'mimic_self'
    | 'mimic_coefficient'
    // Raised by `flatten`, once indices exist, rather than by `validateFrames`.
    | 'too_large'
    | 'unreachable'
    | 'unknown_joint_type'
  /** The frames this is about, in the order they matter. */
  subjects: string[]
  /** One sentence naming what is wrong. */
  detail: string
}

const FRAME_KINDS = ['root', 'static', 'dynamic']

/**
 * Check a frame graph against the rules §44.3 pins.
 *
 * Called before anything derives kinematics from it. Every fault here would otherwise present far
 * from its cause: a missing parent as a frame that silently resolves to identity, a cycle as a hang
 * in the middle of a control cycle, a duplicate name as a limb that moves when a different limb is
 * commanded.
 *
 * Returns null when the graph is usable. It does **not** check that the graph reaches any
 * particular frame — "the tree does not reach the end effector" is a question about a specific
 * guard's needs, and §44.5 has that guard report it with the frame it wanted named.
 */
export function validateFrames(graph: FrameGraph): FrameFault | null {
  if (graph.nodes.length === 0) {
    return { code: 'empty', subjects: [], detail: 'The graph declares no frames.' }
  }

  const seen = new Set<string>()
  for (const node of graph.nodes) {
    if (seen.has(node.name)) {
      return {
        code: 'duplicate_name',
        subjects: [node.name],
        detail: `Two frames are named ${node.name}, so a parent reference is ambiguous.`,
      }
    }
    seen.add(node.name)
  }

  const roots = graph.nodes.filter(node => node.parent === null).map(node => node.name)
  if (roots.length > 1) {
    return {
      code: 'several_roots',
      subjects: roots,
      detail: `${roots.length} frames have no parent, so this is a forest and not a tree.`,
    }
  }
  if (!seen.has(graph.root)) {
    return {
      code: 'root_missing',
      subjects: [graph.root],
      detail: `The declared root ${graph.root} is not one of the frames.`,
    }
  }

  for (const node of graph.nodes) {
    if (!FRAME_KINDS.includes(node.kind)) {
      return {
        code: 'unknown_kind',
        subjects: [node.name],
        detail: `Frame ${node.name} has kind ${node.kind}, which is not root, static or dynamic.`,
      }
    }
    if ((node.kind === 'dynamic') !== (node.joint !== undefined && node.joint !== null)) {
      return {
        code: 'joint_mismatch',
        subjects: [node.name],
        detail:
          node.kind === 'dynamic'
            ? `Frame ${node.name} is dynamic and names no joint.`
            : `Frame ${node.name} is ${node.kind} and names a joint.`,
      }
    }
    if (node.parent !== null && !seen.has(node.parent)) {
      return {
        code: 'parent_missing',
        subjects: [node.name, node.parent],
        detail: `Frame ${node.name} names the parent ${node.parent}, which is not in the graph.`,
      }
    }
  }

  // Every frame reaches the root by following parents. The walk is bounded by the node count, so a
  // cycle is a walk that ran out of frames rather than one that ran forever.
  const byName = new Map(graph.nodes.map(node => [node.name, node]))
  for (const node of graph.nodes) {
    let here = node
    for (let step = 0; step <= graph.nodes.length; step += 1) {
      if (here.parent === null) break
      here = byName.get(here.parent)!
    }
    if (here.parent !== null) {
      return {
        code: 'cycle',
        subjects: [node.name],
        detail: `Following parents from ${node.name} never reaches the root.`,
      }
    }
  }

  // The collision geometry. Checked here rather than by the guard that consumes it, for the same
  // reason as everything above: a shape nobody can read presents as a limb quietly missing from the
  // collision test, and an ignore list naming a frame that does not exist presents as a pair that
  // trips when the operator believed they had excluded it.
  for (const node of graph.nodes) {
    const shape = node.collision_shape
    if (shape !== undefined && shape !== null) {
      if (!COLLISION_SHAPES.includes(shape.kind)) {
        return {
          code: 'unknown_shape',
          subjects: [node.name],
          detail:
            `Frame ${node.name} declares collision geometry of kind ${shape.kind}, which this ` +
            `edition cannot evaluate.`,
        }
      }
      if (!(Number.isFinite(shape.radius_m) && shape.radius_m > 0)) {
        return {
          code: 'unknown_shape',
          subjects: [node.name],
          detail:
            `Frame ${node.name}'s collision geometry has radius ${shape.radius_m}. A capsule of ` +
            `no radius bounds nothing, and a guard built on it would report clearance it has not ` +
            `established.`,
        }
      }
      if (node.collision !== true) {
        return {
          code: 'collision_mismatch',
          subjects: [node.name],
          detail:
            `Frame ${node.name} declares a collision shape and says it carries no collision ` +
            `geometry. One of the two is wrong and nothing here can tell which.`,
        }
      }
    }
    for (const ignored of node.collision_ignore ?? []) {
      if (!graph.nodes.some(other => other.name === ignored)) {
        return {
          code: 'ignore_missing',
          subjects: [node.name, ignored],
          detail:
            `Frame ${node.name} is permitted to touch ${ignored}, which is not one of the frames. ` +
            `An exception naming nothing excludes nothing.`,
        }
      }
    }
  }

  return null
}

/**
 * Check a frame graph against the joint list of the body it describes.
 *
 * `validateFrames` is deliberately graph-only: it is given a tree and asked whether the tree is
 * well formed, and it answers without ever seeing the descriptor the tree came out of. That leaves
 * one whole class of defect uncaught, because the two most important names in a dynamic frame —
 * `joint` and `mimics` — do not refer to anything in the graph. They refer to the joint list.
 *
 * A name that does not resolve there presents far from its cause. The kernel binds channels by
 * joint name, finds nothing for that frame, and `solve` marks it unresolved — so both pose guards
 * trip on it, on every cycle, for as long as the descriptor says what it says. That is the correct
 * behaviour and it is also useless: the robot holds still and the reason is a spelling in a file
 * nothing was checking.
 *
 * §44.5 asks for better than that. A guard which cannot run is supposed to report so **when the
 * lease is granted**, naming the frame, and `staticallyResolvable` is that mechanism — but it reads
 * the joint's *kind*, so it catches a limb hanging below a ball joint and cannot catch a joint name
 * that binds to nothing, because the joint list is not in the graph. This is what closes that gap:
 * it turns a permanent in-cycle trip into a refusal while somebody is still looking at the file.
 *
 * So this is the second half of §44.3, and it is what `embodiment.frames.complete` measures that
 * `validateFrames` cannot. Returns null when every name in the graph resolves.
 *
 * It does **not** require that every declared joint have a frame. A partial tree is legitimate —
 * a gripper whose fingers nobody modelled is a body with joints and no frames for them — and
 * §44.5 already has the guard report unavailable naming the frame it wanted. "Complete" here means
 * every name the graph uses resolves, not that the graph covers the body.
 */
export function validateFrameBindings(
  graph: FrameGraph,
  joints: readonly string[],
): FrameFault | null {
  const declared = new Set(joints)

  for (const node of graph.nodes) {
    if (node.kind === 'dynamic' && node.joint !== undefined && node.joint !== null) {
      if (!declared.has(node.joint)) {
        return {
          code: 'dangling_joint',
          subjects: [node.name, node.joint],
          detail:
            `Frame ${node.name} moves with ${node.joint}, which the body does not declare. ` +
            'Nothing binds it, so its pose never resolves and both pose guards trip on it every ' +
            'cycle.',
        }
      }
    }

    const mimics = node.mimics
    const hasCoefficient = node.mimic_multiplier !== undefined || node.mimic_offset !== undefined

    if (mimics === undefined || mimics === null) {
      if (hasCoefficient) {
        return {
          code: 'mimic_coefficient',
          subjects: [node.name],
          detail:
            `Frame ${node.name} declares a mimic multiplier or offset and no \`mimics\`. ` +
            'A coefficient with nothing to multiply describes a linkage that does not exist.',
        }
      }
      continue
    }

    if (!declared.has(mimics)) {
      return {
        code: 'mimic_missing',
        subjects: [node.name, mimics],
        detail:
          `Frame ${node.name} follows ${mimics}, which the body does not declare. ` +
          'A linkage read from a joint nobody drives never resolves, and takes every frame below ' +
          'it with it.',
      }
    }

    if (node.joint === mimics) {
      return {
        code: 'mimic_self',
        subjects: [node.name, mimics],
        detail:
          `Frame ${node.name} follows the joint it is driven by. ` +
          '`value = multiplier × driver + offset` over one value is a fixed point, not a linkage.',
      }
    }

    for (const [label, value] of [
      ['mimic_multiplier', node.mimic_multiplier],
      ['mimic_offset', node.mimic_offset],
    ] as const) {
      if (value !== undefined && !Number.isFinite(value)) {
        return {
          code: 'mimic_coefficient',
          subjects: [node.name],
          detail:
            `Frame ${node.name} declares ${label} as a value that is not a finite number. ` +
            'A linkage computed from it produces a pose that is not one.',
        }
      }
    }
  }

  return null
}

// ── The source side: what the policy was trained on ──────────────────────────

export const ACTION_KINDS = [
  'joint_position', 'joint_velocity', 'joint_torque',
  'cartesian', 'base_velocity', 'gripper', 'discrete',
] as const
export type ActionKind = (typeof ACTION_KINDS)[number]

export interface ActionChannel {
  /** Position in the policy's output vector. Bindings are returned in this order. */
  index: number
  kind: ActionKind
  /** Canonical role this channel drives, when the publisher declared one. */
  role: string | null
  /** Joint name on the training body, for exact-name matching. */
  joint?: string
  unit?: string
  /**
   * Peak magnitude the channel reached in training, in `unit`. Feeds the clamp: a channel whose
   * peak exceeds the target joint's rating is scaled, and the scaling is reported rather than
   * applied silently.
   */
  peak?: number
  /** Whether the policy still functions with this channel dropped. Defaults to false. */
  optional?: boolean
}

export interface ObservationRequirement {
  name: string
  modality: EmbodimentModality
  role?: string | null
  width?: number
  height?: number
  fps?: number
  optional?: boolean
}

export interface SourceEmbodiment {
  robot_slug?: string | null
  digest?: string | null
  dof?: number
  control_mode: ControlMode
  control_hz?: number
  action_channels: ActionChannel[]
  observations: ObservationRequirement[]
}

// ── The plan ─────────────────────────────────────────────────────────────────

/**
 * How one action channel reaches this body.
 *
 * The six values are ordered by how much they cost in fidelity, and the plan's verdict is
 * derived from the worst one present. Naming `drop` and `blocked` separately matters: dropping a
 * channel the publisher marked optional is a degradation the operator may accept, while blocking
 * a required one is a refusal, and collapsing them would turn a refusal into a shrug.
 */
export const TRANSPORT_METHODS = [
  'direct', 'remap', 'retarget', 'synthesize', 'drop', 'blocked',
] as const
export type TransportMethod = (typeof TRANSPORT_METHODS)[number]

export interface ChannelBinding {
  source_index: number
  source_role: string | null
  method: TransportMethod
  /** Target joint or actuator this channel drives, when one was found. */
  target: string | null
  /**
   * Multiplier applied to the source value before it reaches the target, so a policy trained on
   * a stronger body does not command more than this one is rated for. 1 means no scaling.
   */
  scale: number
  /** Hard ceiling for this channel in the target's units, handed to the Safety Kernel. */
  limit: number | null
  /** English, for logs. A UI translates from `method` plus the subjects. */
  reason: string
}

export interface ObservationBinding {
  required: string
  method: 'direct' | 'remap' | 'downscale' | 'missing'
  target: string | null
  reason: string
}

export type TransportWarningCode =
  | 'no_source_embodiment'
  | 'unverified_embodiment'
  | 'no_target_envelope'
  | 'dof_mismatch'
  | 'control_mode_unsupported'
  | 'rate_unsupported'
  | 'rate_below_watchdog'
  | 'scaled_down'
  | 'unscalable_channel'
  | 'retarget_without_solver'
  | 'unmapped_roles'
  | 'observation_missing'
  | 'observation_degraded'
  | 'latency_undeclared'

export interface TransportWarning {
  code: TransportWarningCode
  message: string
  subjects?: string[]
}

/**
 * Verdicts, worst to best.
 *
 * `refused` is a real refusal: the OS will not install the policy against this body. The other
 * three all run, and the difference between them is what the operator is told they are getting.
 */
export type TransportVerdict = 'direct' | 'retargeted' | 'degraded' | 'refused'

export interface TransportPlan {
  verdict: TransportVerdict
  robot_slug: string
  source_digest: string | null
  target_digest: string | null
  control_mode: ControlMode
  /** Rate the loop will actually run at — the policy's, clamped to what the body accepts. */
  control_hz: number | null
  bindings: ChannelBinding[]
  observations: ObservationBinding[]
  /** Ceilings the Safety Kernel enforces, derived from the envelope and the bindings. */
  clamp: TransportClamp | null
  warnings: TransportWarning[]
}

export interface TransportClamp {
  joint_velocity_max_rad_s?: number
  joint_torque_max_nm?: number
  base_velocity_max_m_s?: number
  min_control_hz?: number
  /** Per-channel ceilings, keyed by target joint name. */
  per_channel: Record<string, number>
  /** Where the envelope figures came from. A limit with no provenance is a rumour. */
  source?: string
}

// ── Resolution ───────────────────────────────────────────────────────────────

const JOINT_KINDS: readonly ActionKind[] = ['joint_position', 'joint_velocity', 'joint_torque']

/** Which joint rating bounds a channel of this kind. */
function ratingFor(kind: ActionKind, limits: JointLimits): number | null {
  if (kind === 'joint_velocity') return limits.velocity_max_rad_s ?? null
  if (kind === 'joint_torque') return limits.effort_max_nm ?? null
  if (kind === 'joint_position') {
    if (limits.position_max_rad === undefined || limits.position_min_rad === undefined) return null
    // Position is bounded by travel rather than by a single magnitude; the ceiling handed to the
    // kernel is the larger absolute bound, and the kernel holds both ends.
    return Math.max(Math.abs(limits.position_max_rad), Math.abs(limits.position_min_rad))
  }
  return null
}

function controlModeFor(kind: ActionKind): ControlMode | null {
  if (kind === 'joint_position') return 'position'
  if (kind === 'joint_velocity') return 'velocity'
  if (kind === 'joint_torque') return 'torque'
  if (kind === 'cartesian') return 'cartesian'
  return null
}

interface TargetIndex {
  byName: Map<string, EmbodimentJoint>
  byRole: Map<string, EmbodimentJoint>
  chains: Set<CanonicalChain>
  /** Roles present on the body that no source channel claimed. Reported, not an error. */
  roles: Set<string>
}

function indexTarget(target: EmbodimentDescriptor): TargetIndex {
  const byName = new Map<string, EmbodimentJoint>()
  const byRole = new Map<string, EmbodimentJoint>()
  const chains = new Set<CanonicalChain>()
  const roles = new Set<string>()

  for (const joint of target.joints) {
    byName.set(joint.name, joint)
    const parsed = parseRole(joint.role)
    if (!parsed) continue
    const key = roleKey(parsed)
    // First declaration wins. A body declaring the same role twice is a descriptor bug, and
    // silently preferring the later one would make the mapping depend on array order.
    if (!byRole.has(key)) byRole.set(key, joint)
    chains.add(parsed.chain)
    roles.add(key)
  }

  return { byName, byRole, chains, roles }
}

export interface ResolveTransportOptions {
  /**
   * Allow task-space retargeting when direct and role matching fail. Off by default: retargeting
   * changes what the policy is doing, and that should be a decision somebody made rather than a
   * fallback that happens quietly.
   */
  allowRetarget?: boolean
  /** Rate the caller intends to run at, when it differs from the policy's declared rate. */
  requestedHz?: number
}

/**
 * Resolve how a policy's action space reaches a body.
 *
 * Pure: no database, no filesystem, no clock. Every input is an argument and the output is a
 * value, which is what lets the Hub and a disconnected robot agree.
 */
export function resolveTransport(
  source: SourceEmbodiment,
  target: EmbodimentDescriptor,
  options: ResolveTransportOptions = {},
): TransportPlan {
  const warnings: TransportWarning[] = []
  const index = indexTarget(target)
  const bindings: ChannelBinding[] = []

  if (!target.verified) {
    warnings.push({
      code: 'unverified_embodiment',
      message: 'This body description is not signed by its vendor. Its joint ratings are unattested.',
      subjects: [target.robot_slug],
    })
  }

  if (source.action_channels.length === 0) {
    warnings.push({
      code: 'no_source_embodiment',
      message: 'The policy declares no action space, so nothing about its fit to this body can be checked.',
    })
  }

  if (target.envelope === null) {
    warnings.push({
      code: 'no_target_envelope',
      message: 'No safety envelope has been declared for this robot. Per-joint ratings are the only ceilings available.',
      subjects: [target.robot_slug],
    })
  }

  if (source.dof !== undefined && source.dof !== target.dof) {
    warnings.push({
      code: 'dof_mismatch',
      message: `The policy was trained on a ${source.dof} degree-of-freedom body; this one has ${target.dof}.`,
    })
  }

  if (!target.control.modes.includes(source.control_mode)) {
    warnings.push({
      code: 'control_mode_unsupported',
      message: `This body does not accept ${source.control_mode} commands.`,
      subjects: [source.control_mode],
    })
  }

  if (target.control.command_latency_ms === undefined) {
    warnings.push({
      code: 'latency_undeclared',
      message: 'This body declares no command latency, so loop stability at the policy rate cannot be predicted.',
    })
  }

  // ── Chains that cannot be satisfied joint-by-joint ────────────────────────
  //
  // Retargeting is a property of a whole kinematic chain, never of one joint: you cannot send an
  // elbow through task space while its shoulder goes direct. So the chains that need it are
  // decided first, and every channel on such a chain is then marked together.
  const retargetChains = decideRetargetChains(source, index, target, options, warnings)

  for (const channel of source.action_channels) {
    bindings.push(bindChannel(channel, source, target, index, retargetChains, warnings))
  }

  const observations = bindObservations(source.observations, target, warnings)
  const controlHz = resolveControlHz(source, target, options, warnings)
  const clamp = buildClamp(target, bindings)
  const verdict = deriveVerdict(bindings)

  const unmapped = [...index.roles].filter(
    role => !bindings.some(binding => binding.source_role && roleKeyOf(binding.source_role) === role),
  )
  if (unmapped.length > 0) {
    warnings.push({
      code: 'unmapped_roles',
      message: `${unmapped.length} joint${unmapped.length === 1 ? '' : 's'} on this body are not driven by the policy and will be held by the controller.`,
      subjects: unmapped,
    })
  }

  return {
    verdict,
    robot_slug: target.robot_slug,
    source_digest: source.digest ?? null,
    target_digest: target.digest,
    control_mode: source.control_mode,
    control_hz: controlHz,
    bindings,
    observations,
    clamp,
    warnings,
  }
}

function roleKeyOf(role: string): string | null {
  const parsed = parseRole(role)
  return parsed ? roleKey(parsed) : null
}

/**
 * Which chains must go through task space, and whether this body can actually do that.
 *
 * A chain qualifies when at least one of its joint channels has no direct or role match. It is
 * only *granted* when the caller asked for retargeting and the body has a solver — otherwise the
 * chain's channels fall through to `blocked`, which is the honest outcome. A plan that promised
 * retargeting on a robot with no IK solver would fail at the first command.
 */
function decideRetargetChains(
  source: SourceEmbodiment,
  index: TargetIndex,
  target: EmbodimentDescriptor,
  options: ResolveTransportOptions,
  warnings: TransportWarning[],
): Set<CanonicalChain> {
  const needsRetarget = new Set<CanonicalChain>()

  for (const channel of source.action_channels) {
    if (!JOINT_KINDS.includes(channel.kind)) continue
    if (channel.joint && index.byName.has(channel.joint)) continue

    const parsed = parseRole(channel.role)
    if (!parsed) continue
    if (index.byRole.has(roleKey(parsed))) continue
    // The chain exists on this body but this particular joint does not — a six-joint arm asked to
    // run a seven-joint policy. That is exactly the case task space can rescue.
    if (index.chains.has(parsed.chain)) needsRetarget.add(parsed.chain)
  }

  if (needsRetarget.size === 0) return needsRetarget

  const canRetarget = options.allowRetarget === true && target.solvers.length > 0
  if (!canRetarget) {
    warnings.push({
      code: 'retarget_without_solver',
      message: options.allowRetarget
        ? 'Task-space retargeting is required but this body has no inverse-kinematics solver installed.'
        : 'Task-space retargeting would be required, and was not requested.',
      subjects: [...needsRetarget],
    })
    return new Set()
  }

  return needsRetarget
}

function bindChannel(
  channel: ActionChannel,
  source: SourceEmbodiment,
  target: EmbodimentDescriptor,
  index: TargetIndex,
  retargetChains: Set<CanonicalChain>,
  warnings: TransportWarning[],
): ChannelBinding {
  const parsed = parseRole(channel.role)

  const base = {
    source_index: channel.index,
    source_role: channel.role,
  }

  // Non-joint channels are matched against the body's own facilities rather than its joints.
  if (!JOINT_KINDS.includes(channel.kind)) {
    return bindNonJointChannel(channel, base, target)
  }

  // 1. Exact wire name. Two vendors agreeing on a joint name is rare and unambiguous when it
  //    happens, so it outranks role matching.
  if (channel.joint) {
    const exact = index.byName.get(channel.joint)
    if (exact) return finishJointBinding(base, channel, exact, 'direct', 'Joint name matches exactly.', warnings)
  }

  // 2. Canonical role. This is the mechanism that makes bodies interoperable at all.
  if (parsed) {
    const byRole = index.byRole.get(roleKey(parsed))
    if (byRole) {
      return finishJointBinding(
        base, channel, byRole, 'remap',
        `Mapped by role ${roleKey(parsed)} to "${byRole.name}".`, warnings,
      )
    }

    // 3. Task space, when the chain was granted it above.
    if (retargetChains.has(parsed.chain)) {
      return {
        ...base,
        method: 'retarget',
        target: null,
        scale: 1,
        limit: null,
        reason: `Driven through task space on the ${parsed.chain} chain; this body has no ${roleKey(parsed)}.`,
      }
    }
  }

  // 4. Optional channels may be dropped; required ones are a refusal.
  if (channel.optional) {
    return {
      ...base,
      method: 'drop',
      target: null,
      scale: 0,
      limit: null,
      reason: 'No equivalent on this body, and the policy declares the channel optional.',
    }
  }

  return {
    ...base,
    method: 'blocked',
    target: null,
    scale: 1,
    limit: null,
    reason: parsed
      ? `This body has no ${roleKey(parsed)} and the policy requires it.`
      : 'The channel declares no canonical role, so it can only match a joint of the same name, and none exists.',
  }
}

function bindNonJointChannel(
  channel: ActionChannel,
  base: { source_index: number; source_role: string | null },
  target: EmbodimentDescriptor,
): ChannelBinding {
  if (channel.kind === 'gripper') {
    const effector = target.end_effectors.find(e => e.kind === 'gripper' || e.kind === 'dexterous_hand')
    if (!effector) {
      return channel.optional
        ? { ...base, method: 'drop', target: null, scale: 0, limit: null, reason: 'This body has no gripper; the channel is optional.' }
        : { ...base, method: 'blocked', target: null, scale: 1, limit: null, reason: 'This body has no gripper and the policy requires one.' }
    }
    return {
      ...base,
      method: 'remap',
      target: effector.name,
      scale: 1,
      limit: effector.max_grip_force_n ?? null,
      reason: `Driven through the ${effector.kind} "${effector.name}".`,
    }
  }

  if (channel.kind === 'base_velocity') {
    if (target.base === 'fixed') {
      return channel.optional
        ? { ...base, method: 'drop', target: null, scale: 0, limit: null, reason: 'This body has a fixed base; the channel is optional.' }
        : { ...base, method: 'blocked', target: null, scale: 1, limit: null, reason: 'The policy commands base motion and this body has a fixed base.' }
    }
    const limit = target.envelope?.base_velocity_max_m_s ?? null
    // A wheeled base driven by a policy trained on a legged one is a synthesis, not a remap: the
    // command means the same thing, and nothing below the base interface is comparable.
    const method: TransportMethod = target.base === 'legged' ? 'remap' : 'synthesize'
    return {
      ...base,
      method,
      target: 'base',
      scale: scaleFor(channel.peak, limit),
      limit,
      reason: `Base velocity applied to this ${target.base} base.`,
    }
  }

  if (channel.kind === 'cartesian') {
    const effector = target.end_effectors.find(e => e.kind !== 'none')
    if (!effector || target.solvers.length === 0) {
      return channel.optional
        ? { ...base, method: 'drop', target: null, scale: 0, limit: null, reason: 'End-effector pose control is unavailable on this body; the channel is optional.' }
        : { ...base, method: 'blocked', target: null, scale: 1, limit: null, reason: 'End-effector pose control needs an end effector and a solver; this body has neither.' }
    }
    return {
      ...base,
      method: 'retarget',
      target: effector.name,
      scale: 1,
      limit: null,
      reason: `Solved onto "${effector.name}" using ${target.solvers[0]}.`,
    }
  }

  // `discrete` — a mode switch or a named action. It carries no physical units, so nothing here
  // can bound it; the package's capability grants are what gate it.
  return {
    ...base,
    method: 'direct',
    target: null,
    scale: 1,
    limit: null,
    reason: 'Discrete command passed through; it commands no actuator directly.',
  }
}

function finishJointBinding(
  base: { source_index: number; source_role: string | null },
  channel: ActionChannel,
  joint: EmbodimentJoint,
  method: TransportMethod,
  reason: string,
  warnings: TransportWarning[],
): ChannelBinding {
  const rating = ratingFor(channel.kind, joint.limits)
  const scale = scaleFor(channel.peak, rating)
  const mode = controlModeFor(channel.kind)

  if (mode && !joint.control_modes.includes(mode)) {
    return {
      ...base,
      method: 'blocked',
      target: joint.name,
      scale: 1,
      limit: rating,
      reason: `"${joint.name}" does not accept ${mode} commands.`,
    }
  }

  if (scale < 1) {
    warnings.push({
      code: 'scaled_down',
      message: `Commands to "${joint.name}" are scaled to ${(scale * 100).toFixed(0)}% because the policy was trained on a stronger joint.`,
      subjects: [joint.name],
    })
  } else if (channel.peak !== undefined && rating === null) {
    warnings.push({
      code: 'unscalable_channel',
      message: `"${joint.name}" declares no rating for this control mode, so the policy's commands cannot be scaled to it.`,
      subjects: [joint.name],
    })
  }

  return { ...base, method, target: joint.name, scale, limit: rating, reason }
}

/**
 * How far a channel must be scaled to stay inside a joint's rating.
 *
 * Returns 1 when either number is missing, and a warning is raised separately. Guessing a ratio
 * from one known side would be inventing the other, which is precisely what this module refuses
 * to do anywhere else.
 */
function scaleFor(peak: number | undefined, rating: number | null): number {
  if (peak === undefined || rating === null) return 1
  if (peak <= 0) return 1
  return Math.min(1, rating / peak)
}

function bindObservations(
  required: ObservationRequirement[],
  target: EmbodimentDescriptor,
  warnings: TransportWarning[],
): ObservationBinding[] {
  return required.map(requirement => {
    const exact = target.sensors.find(s => s.name === requirement.name && s.modality === requirement.modality)
    const candidate = exact ?? matchSensorByRole(requirement, target)

    if (!candidate) {
      if (!requirement.optional) {
        warnings.push({
          code: 'observation_missing',
          message: `The policy reads a ${requirement.modality} stream called "${requirement.name}" and this body has none.`,
          subjects: [requirement.name],
        })
      }
      return {
        required: requirement.name,
        method: 'missing' as const,
        target: null,
        reason: requirement.optional
          ? 'Not present on this body; the policy declares it optional.'
          : 'Not present on this body.',
      }
    }

    const degraded =
      (requirement.width !== undefined && (candidate.width ?? 0) < requirement.width) ||
      (requirement.height !== undefined && (candidate.height ?? 0) < requirement.height) ||
      (requirement.fps !== undefined && (candidate.fps ?? 0) < requirement.fps)

    if (degraded) {
      warnings.push({
        code: 'observation_degraded',
        message: `"${candidate.name}" is lower resolution or slower than the stream the policy was trained on.`,
        subjects: [candidate.name],
      })
      return {
        required: requirement.name,
        method: 'downscale' as const,
        target: candidate.name,
        reason: 'Present but below the trained resolution or frame rate; frames are upscaled.',
      }
    }

    return {
      required: requirement.name,
      method: exact ? ('direct' as const) : ('remap' as const),
      target: candidate.name,
      reason: exact ? 'Sensor name and modality match.' : `Matched by role to "${candidate.name}".`,
    }
  })
}

function matchSensorByRole(
  requirement: ObservationRequirement,
  target: EmbodimentDescriptor,
): EmbodimentSensor | null {
  const parsed = parseRole(requirement.role)
  if (!parsed) return null
  const key = roleKey(parsed)
  return (
    target.sensors.find(sensor => {
      if (sensor.modality !== requirement.modality) return false
      const sensorRole = parseRole(sensor.role)
      return sensorRole !== null && roleKey(sensorRole) === key
    }) ?? null
  )
}

/**
 * The rate the loop will actually run at.
 *
 * Clamped down to what the body accepts, never up: running a policy faster than it was trained
 * changes its dynamics, and running it slower is a documented degradation rather than a silent
 * one. The watchdog check is the one that matters most — a loop slower than the controller's
 * timeout faults *while the robot is moving*, which is worse than refusing to start.
 */
function resolveControlHz(
  source: SourceEmbodiment,
  target: EmbodimentDescriptor,
  options: ResolveTransportOptions,
  warnings: TransportWarning[],
): number | null {
  const requested = options.requestedHz ?? source.control_hz
  if (requested === undefined) return null

  let resolved = requested
  const max = target.control.rate_hz_max
  if (max !== undefined && requested > max) {
    resolved = max
    warnings.push({
      code: 'rate_unsupported',
      message: `The policy runs at ${requested} Hz; this body accepts at most ${max} Hz, so the loop is clamped.`,
    })
  }

  const watchdogMs = target.envelope?.watchdog_ms
  if (watchdogMs !== undefined && watchdogMs > 0) {
    const minHz = 1000 / watchdogMs
    if (resolved < minHz) {
      warnings.push({
        code: 'rate_below_watchdog',
        message: `At ${resolved} Hz the controller's ${watchdogMs} ms watchdog fires between commands.`,
      })
    }
  }

  return resolved
}

function buildClamp(
  target: EmbodimentDescriptor,
  bindings: ChannelBinding[],
): TransportClamp | null {
  const envelope = target.envelope
  const perChannel: Record<string, number> = {}

  for (const binding of bindings) {
    if (binding.target && binding.limit !== null) perChannel[binding.target] = binding.limit
  }

  if (!envelope && Object.keys(perChannel).length === 0) return null

  const watchdogMs = envelope?.watchdog_ms
  return {
    ...(envelope?.joint_velocity_max_rad_s !== undefined && { joint_velocity_max_rad_s: envelope.joint_velocity_max_rad_s }),
    ...(envelope?.joint_torque_max_nm !== undefined && { joint_torque_max_nm: envelope.joint_torque_max_nm }),
    ...(envelope?.base_velocity_max_m_s !== undefined && { base_velocity_max_m_s: envelope.base_velocity_max_m_s }),
    ...(watchdogMs !== undefined && watchdogMs > 0 && { min_control_hz: 1000 / watchdogMs }),
    per_channel: perChannel,
    ...(envelope?.source !== undefined && { source: envelope.source }),
  }
}

/**
 * The plan's verdict is the worst binding in it.
 *
 * One blocked required channel refuses the whole plan, because a policy missing an actuator it
 * depends on does not do most of its job — it does something nobody has characterised.
 */
export function deriveVerdict(bindings: ChannelBinding[]): TransportVerdict {
  if (bindings.some(b => b.method === 'blocked')) return 'refused'
  if (bindings.some(b => b.method === 'drop')) return 'degraded'
  if (bindings.some(b => b.method === 'retarget' || b.method === 'synthesize')) return 'retargeted'
  return 'direct'
}

/** Whether the OS will install a policy under this plan. Refusals are the only hard stop. */
export function isInstallable(plan: TransportPlan): boolean {
  return plan.verdict !== 'refused'
}
