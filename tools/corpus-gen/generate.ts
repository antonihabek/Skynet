/**
 * Generate the conformance corpus from the reference edition.
 *
 * §31.4: the normative artifact is not a codebase, it is **the specification plus a corpus of
 * golden vectors that every edition must reproduce exactly**. This script produces that corpus by
 * running curated inputs through the TypeScript oracle and freezing the canonical bytes of each
 * output.
 *
 * Four properties of the output format, each load-bearing:
 *
 * 1. **`expected` is a string, not an object.** The requirement is byte-identical output, so the
 *    bytes are what the file holds. A reviewer can see exactly what an edition must emit, and a
 *    runner compares strings rather than walking two trees and deciding what "equal" means.
 * 2. **`input` is a plain object**, pretty-printed, because a human has to read it to judge
 *    whether the case is testing what its note claims.
 * 3. **Every case carries a note.** A vector whose purpose nobody recorded is a vector nobody dares
 *    change when it fails.
 * 4. **Nothing is random at generation time.** The fuzz corpus uses a seeded generator written out
 *    in full below, so regenerating produces byte-identical files and a diff means a behaviour
 *    change rather than a reshuffle.
 *
 * Run: `node tools/corpus-gen/generate.ts`
 */

import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { createHash } from 'node:crypto'

import { canonical } from '../../reference/typescript/canonical.ts'
import {
  encodeHeader, encodeRecord, hex, opcodes, recordSize, wire, writeMcap,
  type Cycle, type Entry as BlackboxEntry, type Geometry as RingGeometry,
  type Meta as RingMeta, type Shape as RingShape,
} from '../../reference/typescript/blackbox.ts'
import {
  parseRole, resolveTransport, roleKey, validateFrames,
  type ActionChannel, type EmbodimentDescriptor, type FrameGraph, type FrameNode,
  type ResolveTransportOptions, type SourceEmbodiment,
} from '../../reference/typescript/embodiment.ts'
import { admitPackage, type AdmissionParams } from '../../reference/typescript/package.ts'
import { versionDigest, type DigestFile } from '../../reference/typescript/digest.ts'
import {
  awardCertification, testsRequiredFor, type CertificationInput,
} from '../../reference/typescript/conformance.ts'
import {
  evaluateReadiness, summarizeReadiness, type ReadinessSubject,
} from '../../reference/typescript/readiness.ts'
import {
  evaluateCycle, resolveLimits, withGrants,
  type CommandCycle, type ResolveLimitsParams,
} from '../../reference/typescript/safety.ts'
import type { SafetyEnvelope } from '../../reference/typescript/types.ts'
import {
  shapeAdmission, shapeCertification, shapeFleet, shapeFrameFault, shapeLimits, shapePlan,
  shapeDigest, shapeReadiness, shapeSolved, shapeVerdict,
} from '../../reference/typescript/shape.ts'
import {
  flatten, solve,
  type Frame, type JointBinding, type WorkspaceLayer,
} from '../../reference/typescript/kinematics.ts'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const CORPUS = join(ROOT, 'corpus')

interface Case {
  id: string
  note: string
  input: unknown
  expected: string
}

const files = new Map<string, Case[]>()

function add(corpus: string, file: string, entry: Case): void {
  const key = `${corpus}/${file}`
  const list = files.get(key) ?? []
  list.push(entry)
  files.set(key, list)
}

// ── Fixtures ─────────────────────────────────────────────────────────────────
//
// One arm and one humanoid, reused across corpora so a reader comparing a transport case with an
// admission case is looking at the same robot.

function joint(
  name: string, role: string | null, effort: number | null, modes: string[],
): EmbodimentDescriptor['joints'][number] {
  return {
    name,
    role,
    type: 'revolute',
    limits: {
      position_min_rad: -3,
      position_max_rad: 3,
      velocity_max_rad_s: 2,
      ...(effort === null ? {} : { effort_max_nm: effort }),
    },
    control_modes: modes,
  }
}

const ARM: EmbodimentDescriptor = {
  schema_version: 1,
  robot_slug: 'test-arm',
  digest: 'sha256:arm',
  verified: true,
  base: 'fixed',
  dof: 3,
  joints: [
    joint('J1', 'right_arm.shoulder.pitch', 80, ['torque', 'position']),
    joint('J2', 'right_arm.elbow.flex', 60, ['torque', 'position']),
    joint('J3', 'right_arm.wrist.roll', 20, ['position']),
  ],
  end_effectors: [{
    name: 'hand', role: 'right_hand.gripper.grip', kind: 'gripper',
    dof: 1, max_grip_force_n: 40, max_opening_m: 0.08,
  }],
  sensors: [{
    name: 'wrist_cam', role: 'right_arm.wrist.roll', modality: 'rgb',
    width: 640, height: 480, fps: 30, frame: 'right_arm.tool0',
  }],
  control: { modes: ['position', 'torque'], rate_hz_max: 200, rate_hz_min: 10, command_latency_ms: 4 },
  envelope: {
    joint_velocity_max_rad_s: 2, joint_torque_max_nm: 80,
    watchdog_ms: 100, estop: true, source: 'https://example.invalid/arm.pdf',
  },
  solvers: ['trac_ik'],
}

const HUMANOID: EmbodimentDescriptor = {
  schema_version: 1,
  robot_slug: 'test-humanoid',
  digest: 'sha256:humanoid',
  verified: false,
  base: 'legged',
  dof: 5,
  joints: [
    joint('l_sh_p', 'left_arm.shoulder.pitch', 120, ['torque']),
    joint('l_el_f', 'left_arm.elbow.flex', 90, ['torque']),
    joint('r_sh_p', 'right_arm.shoulder.pitch', 120, ['torque']),
    joint('r_el_f', 'right_arm.elbow.flex', 90, ['torque']),
    joint('waist', 'torso.yaw', 200, ['torque', 'position']),
  ],
  end_effectors: [],
  sensors: [
    { name: 'head_rgb', role: 'head.camera.rotate', modality: 'rgb', width: 1280, height: 720, fps: 30 },
    { name: 'torso_imu', role: 'torso.imu', modality: 'imu' },
  ],
  control: { modes: ['torque'], rate_hz_max: 500, rate_hz_min: 50 },
  envelope: null,
  solvers: [],
}

function channel(
  index: number, kind: string, role: string | null,
  extra: Partial<ActionChannel> = {},
): ActionChannel {
  return { index, kind: kind as ActionChannel['kind'], role, ...extra }
}

function policy(
  channels: ActionChannel[], extra: Partial<SourceEmbodiment> = {},
): SourceEmbodiment {
  return {
    robot_slug: 'trainer',
    digest: 'sha256:policy',
    dof: 3,
    control_mode: 'torque',
    control_hz: 50,
    action_channels: channels,
    observations: [],
    ...extra,
  }
}

// ── roles ────────────────────────────────────────────────────────────────────

function roleCase(id: string, note: string, role: string | null): void {
  const parsed = parseRole(role)
  add('roles', 'vocabulary', {
    id,
    note,
    input: { role },
    expected: canonical(parsed === null ? null : {
      chain: parsed.chain,
      link: parsed.link,
      axis: parsed.axis,
      index: parsed.index,
      key: roleKey(parsed),
    }),
  })
}

roleCase('three-segment', 'The full form: chain, link and axis.', 'left_arm.shoulder.pitch')
roleCase('two-segment', 'Two segments means the second is the link, not the axis.', 'torso.yaw')
roleCase('ordinal', 'An ordinal distinguishes repeated links.', 'right_hand.finger.flex#2')
roleCase('ordinal-zero', 'Zero is a valid ordinal and must not read as absent.', 'base.wheel#0')
roleCase('normalised-case', 'Input is lowercased before parsing.', 'LEFT_ARM.Shoulder.PITCH')
roleCase('normalised-space', 'Input is trimmed before parsing.', '  base.wheel#1  ')
roleCase('aux-chain', 'A body plan the vocabulary did not anticipate still parses under aux.', 'aux.mast.pitch')
roleCase('digit-in-link', 'Links may carry digits; chains may not.', 'base.wheel2')
roleCase('unknown-chain', 'An unrecognised chain is null, never a nearby guess.', 'tentacle.segment.flex')
roleCase('chain-with-digit', 'A digit in the chain fails the charset.', 'base2.wheel')
roleCase('unknown-axis', 'An unrecognised axis is null.', 'left_arm.shoulder.wiggle')
roleCase('one-segment', 'One segment is not a role.', 'left_arm')
roleCase('four-segments', 'The grammar has three segments.', 'a.b.c.d')
roleCase('empty-link', 'An empty segment fails.', 'left_arm..pitch')
roleCase('empty-ordinal', 'A bare # is not an ordinal.', 'base.wheel#')
roleCase('non-numeric-ordinal', 'An ordinal must be digits.', 'base.wheel#a')
roleCase('double-ordinal', 'Two ordinals cannot both be the last thing.', 'base.wheel#1#2')
roleCase('hyphen', 'Hyphens are outside every charset.', 'left_arm.shoulder-x.pitch')
roleCase('digit-in-axis', 'Axes are letters only.', 'left_arm.shoulder.pitch2')
roleCase('null-role', 'A joint with no declared role parses to null.', null)
roleCase('empty-string', 'An empty role is null, not an error.', '')
roleCase('whitespace-only', 'Whitespace trims to nothing.', '   ')
for (const chain of ['left_arm', 'right_leg', 'left_hand', 'neck', 'head', 'base', 'tail', 'aux']) {
  roleCase(`chain-${chain}`, `${chain} is in the vocabulary.`, `${chain}.link.pitch`)
}
for (const axis of ['pitch', 'yaw', 'roll', 'slide', 'flex', 'abduct', 'rotate', 'grip']) {
  roleCase(`axis-${axis}`, `${axis} is in the vocabulary.`, `torso.link.${axis}`)
}

// ── transport ────────────────────────────────────────────────────────────────

function transportCase(
  file: string, id: string, note: string,
  source: SourceEmbodiment, target: EmbodimentDescriptor,
  options: ResolveTransportOptions = {},
): void {
  add('transport', file, {
    id,
    note,
    input: { source, target, options },
    expected: canonical(shapePlan(resolveTransport(source, target, options))),
  })
}

transportCase('binding', 'exact-name-wins',
  'An exact wire name outranks a role match, because two vendors agreeing on a name is unambiguous.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex', { joint: 'J1' })]), ARM)

transportCase('binding', 'role-remap',
  'Different wire names, same canonical role: the mechanism that makes bodies interoperable.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex', { joint: 'elbow_joint' })]), ARM)

transportCase('binding', 'blocked-required',
  'A required channel with no equivalent refuses the whole plan.',
  policy([channel(0, 'joint_torque', 'left_leg.knee.flex')]), ARM)

transportCase('binding', 'dropped-optional',
  'The same channel marked optional degrades instead of refusing.',
  policy([channel(0, 'joint_torque', 'left_leg.knee.flex', { optional: true })]), ARM)

transportCase('binding', 'mode-refused',
  'A joint that matched but does not accept the mode is blocked, with the joint still named.',
  policy([channel(0, 'joint_torque', 'right_arm.wrist.roll')]), ARM)

transportCase('binding', 'no-role-no-name',
  'A channel with neither a role nor a matching name can only be blocked.',
  policy([channel(0, 'joint_torque', null, { joint: 'nonexistent' })]), ARM)

transportCase('binding', 'position-channel-bounds-by-travel',
  'A position channel is bounded by the larger absolute travel limit.',
  policy([channel(0, 'joint_position', 'right_arm.elbow.flex', { peak: 6 })],
    { control_mode: 'position' }), ARM)

transportCase('scaling', 'scaled-down',
  'A policy trained at 180 Nm on an 80 Nm joint is scaled and the operator is told the figure.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.pitch', { peak: 180, unit: 'Nm' })]), ARM)

transportCase('scaling', 'not-scaled-up',
  'A weaker policy is never scaled up to fill a stronger joint.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.pitch', { peak: 10 })]), ARM)

transportCase('scaling', 'unscalable',
  'A joint with no rating cannot be scaled to, and the gap is reported rather than filled.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.pitch', { peak: 180 })]),
  { ...ARM, joints: [joint('J1', 'right_arm.shoulder.pitch', null, ['torque']), ...ARM.joints.slice(1)] })

transportCase('scaling', 'peak-zero',
  'A zero peak cannot produce a ratio and is left alone.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.pitch', { peak: 0 })]), ARM)

transportCase('retarget', 'not-requested',
  'Retargeting is off by default and the message says it was not requested.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.yaw')]), ARM)

transportCase('retarget', 'granted',
  'Asked for, and the body has a solver.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.yaw')]), ARM, { allowRetarget: true })

transportCase('retarget', 'no-solver',
  'Asked for, and the body has none — refused rather than promised.',
  policy([channel(0, 'joint_torque', 'right_arm.shoulder.yaw')]),
  { ...ARM, solvers: [] }, { allowRetarget: true })

transportCase('retarget', 'whole-chain',
  'Retargeting is a chain-level decision: you cannot send an elbow through task space while its shoulder goes direct.',
  policy([
    channel(0, 'joint_torque', 'right_arm.shoulder.pitch'),
    channel(1, 'joint_torque', 'right_arm.shoulder.yaw'),
    channel(2, 'joint_torque', 'right_arm.elbow.flex'),
  ]), ARM, { allowRetarget: true })

transportCase('non-joint', 'gripper',
  'A gripper channel finds the end effector and inherits its force rating.',
  policy([channel(0, 'gripper', null)]), ARM)

transportCase('non-joint', 'gripper-absent',
  'No gripper and the policy requires one.',
  policy([channel(0, 'gripper', null)]), HUMANOID)

transportCase('non-joint', 'base-fixed',
  'Base motion commanded at a body bolted to a bench.',
  policy([channel(0, 'base_velocity', null)]), ARM)

transportCase('non-joint', 'base-legged-remap',
  'Legged to legged is a remap.',
  policy([channel(0, 'base_velocity', null)]), HUMANOID)

transportCase('non-joint', 'base-wheeled-synthesize',
  'Legged policy onto a wheeled base: the meaning transfers, the mechanism does not.',
  policy([channel(0, 'base_velocity', null, { peak: 3 })]),
  { ...HUMANOID, base: 'wheeled', envelope: { base_velocity_max_m_s: 1.2, watchdog_ms: 20 } })

transportCase('non-joint', 'cartesian',
  'End-effector pose control needs both an effector and a solver.',
  policy([channel(0, 'cartesian', null)]), ARM)

transportCase('non-joint', 'cartesian-no-solver',
  'Without a solver it is blocked, not attempted.',
  policy([channel(0, 'cartesian', null)]), { ...ARM, solvers: [] })

transportCase('non-joint', 'discrete',
  'A discrete command carries no physical units, so nothing here can bound it.',
  policy([channel(0, 'discrete', null)]), ARM)

transportCase('observation', 'exact',
  'Name and modality match.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], {
    observations: [{ name: 'wrist_cam', modality: 'rgb', width: 640, height: 480, fps: 30 }],
  }), ARM)

transportCase('observation', 'by-role',
  'Matched by role when the name differs.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], {
    observations: [{ name: 'eye_in_hand', modality: 'rgb', role: 'right_arm.wrist.roll' }],
  }), ARM)

transportCase('observation', 'downscale',
  'Present but poorer than the stream the policy was trained on.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], {
    observations: [{ name: 'wrist_cam', modality: 'rgb', width: 1920, height: 1080, fps: 60 }],
  }), ARM)

transportCase('observation', 'missing-required',
  'A required stream with no equivalent.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], {
    observations: [{ name: 'head_cam', modality: 'depth' }],
  }), ARM)

transportCase('observation', 'missing-optional',
  'The same stream, declared optional: no warning.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], {
    observations: [{ name: 'head_cam', modality: 'depth', optional: true }],
  }), ARM)

transportCase('rate', 'clamped-down',
  'The loop is clamped to what the body accepts.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], { control_hz: 1000 }), ARM)

transportCase('rate', 'below-watchdog',
  'A loop slower than the controller timeout faults while the robot is moving — the warning that matters most.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], { control_hz: 5 }), ARM)

transportCase('rate', 'requested-override',
  'The caller may name a rate different from the policy\'s own.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')]), ARM, { requestedHz: 400 })

transportCase('rate', 'undeclared',
  'A policy that declares no rate produces no rate.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], { control_hz: undefined }), ARM)

transportCase('declaration', 'unverified-body',
  'An unsigned descriptor is usable and flagged.',
  policy([channel(0, 'joint_torque', 'left_arm.elbow.flex')], { dof: 5 }), HUMANOID)

transportCase('declaration', 'no-envelope',
  'No whole-body envelope; per-joint ratings survive as the only ceilings.',
  policy([channel(0, 'joint_torque', 'left_arm.elbow.flex')], { dof: 5 }), HUMANOID)

transportCase('declaration', 'no-clamp-at-all',
  'Neither an envelope nor a bound channel: no clamp, rather than an empty one.',
  policy([channel(0, 'discrete', null)], { dof: 5 }), HUMANOID)

transportCase('declaration', 'dof-mismatch',
  'Both figures appear in the sentence.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], { dof: 7 }), ARM)

transportCase('declaration', 'mode-unsupported',
  'The body does not accept the mode the policy emits.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], { control_mode: 'trajectory' }), ARM)

transportCase('declaration', 'empty-action-space',
  'Nothing about the fit can be checked, and the plan says so.',
  policy([]), ARM)

transportCase('declaration', 'unmapped-roles',
  'Joints the policy does not drive are named, so the operator knows what the controller will hold.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')]), ARM)

transportCase('declaration', 'null-digests',
  'A body and a policy with no digests still plan; the nulls are facts, not gaps.',
  policy([channel(0, 'joint_torque', 'right_arm.elbow.flex')], { digest: null }),
  { ...ARM, digest: null })

// ── cycle ────────────────────────────────────────────────────────────────────

/**
 * A body, as a corpus case carries it.
 *
 * The **graph** goes into the input, not the flattened frames: a case file is read by a human and
 * consumed by every edition, and §44.3's graph is the shape a descriptor actually has. Flattening
 * is itself part of what the corpus checks, so a case that pre-flattened would be skipping the step
 * two editions are most likely to disagree about.
 */
interface Body {
  frames?: FrameGraph
  joints?: JointBinding[]
  grants?: string[]
}

function flattenOrThrow(graph: FrameGraph): Frame[] {
  const result = flatten(graph)
  if ('fault' in result) {
    throw new Error(`fixture frame graph is invalid: ${result.fault.code} — ${result.fault.detail}`)
  }
  return result.frames
}

function cycleCase(
  file: string, id: string, note: string,
  limitsParams: ResolveLimitsParams, cycle: CommandCycle,
  lastSequence?: number, body: Body = {},
): void {
  const frames = body.frames ? flattenOrThrow(body.frames) : null
  const limits = withGrants(
    resolveLimits({ ...limitsParams, frames }),
    body.grants ?? [],
  )
  const verdict = evaluateCycle(cycle, limits, limitsParams.envelope, lastSequence, body.joints ?? [])
  add('cycle', file, {
    id,
    note,
    input: {
      limits: { ...limitsParams, ...(body.frames ? { frames: body.frames } : {}), ...(body.grants ? { grants: body.grants } : {}) },
      cycle,
      last_sequence: lastSequence === undefined ? null : lastSequence,
      ...(body.joints ? { joints: body.joints } : {}),
    },
    expected: canonical(shapeVerdict(verdict)),
  })
}

const ENVELOPE: SafetyEnvelope = {
  joint_velocity_max_rad_s: 2,
  joint_torque_max_nm: 80,
  base_velocity_max_m_s: 1.5,
  watchdog_ms: 100,
  estop: true,
  source: 'https://example.invalid/datasheet.pdf',
}

const CLAMP = {
  joint_velocity_max_rad_s: 2,
  joint_torque_max_nm: 80,
  base_velocity_max_m_s: 1.5,
  min_control_hz: 10,
  per_channel: { elbow: 80, shoulder: 60 },
  source: 'https://example.invalid/datasheet.pdf',
}

const ALL_AVAILABLE = { imu: true, people_detection: true, legged: true }

function hostParams(overrides: Partial<ResolveLimitsParams> = {}): ResolveLimitsParams {
  return {
    tier: 'host',
    envelope: ENVELOPE,
    transport: CLAMP,
    profile: { name: 'cell-a', max_delta_per_cycle: 0.25 },
    available: ALL_AVAILABLE,
    ...overrides,
  }
}

function cycle(desired: Record<string, number>, extra: Partial<CommandCycle> = {}): CommandCycle {
  return { now_ms: 1000, command_ms: 995, sequence: 42, desired, ...extra }
}

cycleCase('guards', 'inside-every-ceiling',
  'A command inside every ceiling passes through untouched.',
  hostParams(), cycle({ elbow: 40, shoulder: 10 }))

cycleCase('guards', 'clamped-to-rating',
  'Above the joint rating, clamped to it.',
  hostParams(), cycle({ elbow: 120 }))

cycleCase('guards', 'clamp-preserves-sign',
  'Clamping reduces magnitude and keeps direction.',
  hostParams(), cycle({ elbow: -120 }))

cycleCase('guards', 'stale-command-discarded',
  'A stale command is discarded, never clamped and forwarded at a legal magnitude.',
  hostParams(), cycle({ elbow: 500 }, { command_ms: 800 }))

cycleCase('guards', 'stalled-sequence',
  'A sequence that did not advance is a stalled producer.',
  hostParams(), cycle({ elbow: 10 }), 42)

cycleCase('guards', 'advancing-sequence',
  'An advancing sequence is not a stall.',
  hostParams(), cycle({ elbow: 10 }), 41)

cycleCase('guards', 'no-watchdog-window',
  'A guard with no input cannot fire; it is reported unavailable instead.',
  hostParams({ envelope: { ...ENVELOPE, watchdog_ms: undefined } }),
  cycle({ elbow: 10 }, { command_ms: 0 }), 999)

cycleCase('guards', 'rate-limit-after-clamp',
  'Clamping creates a step change; the rate limiter is what turns it into a ramp.',
  hostParams(), cycle({ elbow: 400 }, { previous: { elbow: 0 } }))

cycleCase('guards', 'rate-limit-unbounded-channel',
  'A channel with no ceiling rate-limits against its own previous value.',
  hostParams(), cycle({ wrist: 100 }, { previous: { wrist: 10 } }))

cycleCase('guards', 'rate-limit-not-configured',
  'With no jerk ceiling a single-cycle jump to the clamped maximum is permitted, and the notice says so.',
  hostParams({ profile: null }), cycle({ elbow: 400 }, { previous: { elbow: 0 } }))

cycleCase('guards', 'base-channel',
  'The reserved base channel is bounded by the base velocity ceiling.',
  hostParams(), cycle({ base: 9 }))

cycleCase('guards', 'multi-channel',
  'Several channels, some clamped and some not, in one cycle.',
  hostParams(), cycle({ base: 4, elbow: 200, shoulder: 10, wrist: 3 }))

cycleCase('advisory', 'forwards-unchanged',
  'At the guest tier the verdict is computed and the command still reaches the robot unchanged.',
  hostParams({ tier: 'guest' }), cycle({ elbow: 120 }))

cycleCase('advisory', 'stop-still-forwards',
  'There is nothing underneath a guest-tier deployment to stop.',
  hostParams({ tier: 'guest' }), cycle({ elbow: 10 }, { command_ms: 0 }))

cycleCase('advisory', 'certified-tier',
  'The certified tier resolves to redundant enforcement.',
  hostParams({ tier: 'certified' }), cycle({ elbow: 120 }))

cycleCase('empty', 'no-channels',
  'An empty command is not an error.',
  hostParams(), cycle({}))

// ── The two guards the reference could not previously exercise ──────────────
//
// `proximity` and `mode_gate` were implemented in the Rust edition and absent from this one, and
// nothing caught it: the generator never produced a cycle carrying a capability token or a
// separation distance, so the corpus compared the two editions on inputs where both did nothing.
// Two of the eight guards were therefore pinned by one edition's own suite, which is the
// arrangement §31.4 exists to prevent. The `CommandCycle` now carries both fields.

const SHARED_SPACE = {
  name: 'shared-space',
  max_delta_per_cycle: 0.25,
  proximity: { stop_distance_m: 0.3, slow_distance_m: 1, slow_factor: 0.5 },
}

cycleCase('proximity', 'inside-the-stop-distance',
  'A person inside the protective separation distance is a category 1 stop, and nothing is sent.',
  hostParams({ profile: SHARED_SPACE }), cycle({ elbow: 40 }, { nearest_person_m: 0.2 }))

cycleCase('proximity', 'between-the-two-distances',
  'Speed and separation monitoring scales the command and forwards it — a reduction, reported as a clamp and not as a stop.',
  hostParams({ profile: SHARED_SPACE }), cycle({ elbow: 40 }, { nearest_person_m: 0.8 }))

cycleCase('proximity', 'beyond-the-slow-distance',
  'Outside both distances the guard is active and does nothing.',
  hostParams({ profile: SHARED_SPACE }), cycle({ elbow: 40 }, { nearest_person_m: 4 }))

cycleCase('proximity', 'scales-after-clamping',
  'The separation factor applies to the clamped command, so a clamp cannot raise a speed the separation reduced.',
  hostParams({ profile: SHARED_SPACE }), cycle({ elbow: 400 }, { nearest_person_m: 0.8 }))

cycleCase('proximity', 'unsensed',
  'Configured but nothing measures separation: the guard is unavailable and the distance field is ignored.',
  hostParams({ profile: SHARED_SPACE, available: { ...ALL_AVAILABLE, people_detection: false } }),
  cycle({ elbow: 40 }, { nearest_person_m: 0.1 }))

cycleCase('mode-gate', 'granted',
  'A capability the operator granted passes.',
  hostParams(), cycle({ elbow: 40 }, { capability: 'motion.arm' }), undefined,
  { grants: ['motion.arm'] })

cycleCase('mode-gate', 'not-granted',
  'A capability the operator did not grant holds the command; the revocation takes effect on the next cycle, not the next restart.',
  hostParams(), cycle({ elbow: 40 }, { capability: 'motion.base' }), undefined,
  { grants: ['motion.arm'] })

cycleCase('mode-gate', 'no-capability-named',
  'A cycle that names no capability does not evaluate the gate — absent is not "no capability".',
  hostParams(), cycle({ elbow: 40 }), undefined, { grants: ['motion.arm'] })

// ── The pose guards (§7.1 rows four and five, §44) ──────────────────────────

function poseFrame(node: Partial<FrameNode> & { name: string }): FrameNode {
  return { parent: null, kind: 'root', ...node } as FrameNode
}

/** A one-metre arm: a hinge about z, a tool at its end, and a capsule down its length. */
const POSE_ARM: FrameGraph = {
  root: 'base_link',
  convention: 'rep103',
  generator: 'urdf',
  nodes: [
    poseFrame({ name: 'base_link' }),
    poseFrame({
      name: 'upper', parent: 'base_link', kind: 'dynamic',
      translation: [0, 0, 0], rotation: [0, 0, 0, 1],
      joint: 'shoulder', joint_type: 'revolute', axis: [0, 0, 1],
      collision: true,
      collision_shape: { kind: 'capsule', a: [0, 0, 0], b: [1, 0, 0], radius_m: 0.05 },
    }),
    poseFrame({
      name: 'tool', parent: 'upper', kind: 'static',
      translation: [1, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
    }),
  ],
}

const POSE_ARM_JOINTS: JointBinding[] = [
  { joint: 'shoulder', channel: 'shoulder', measured: 0 },
]

function layer(name: string, volumes: WorkspaceLayer['volumes']): WorkspaceLayer {
  return { name, volumes }
}

const INSIDE = layer('cell', [
  { frame: 'tool', shape: { kind: 'sphere', centre: [0, 0, 0], radius_m: 2 } },
])
const TIGHT = layer('cell', [
  { frame: 'tool', shape: { kind: 'sphere', centre: [0, 0, 0], radius_m: 0.5 } },
])
const SLAB = layer('cell', [
  { frame: 'tool', shape: { kind: 'box', min: [0.5, -0.2, -0.2], max: [1.5, 0.2, 0.2] } },
])

function poseProfile(overrides: Record<string, unknown> = {}) {
  return { name: 'cell-a', max_delta_per_cycle: 0.25, ...overrides }
}

cycleCase('workspace', 'inside-the-volume',
  'A commanded pose inside the allowed volume passes, and the verdict records that a lookahead was what ran.',
  hostParams({ profile: poseProfile({ workspace: [INSIDE] }) }),
  cycle({ shoulder: 0 }), undefined, { frames: POSE_ARM, joints: POSE_ARM_JOINTS })

cycleCase('workspace', 'outside-every-volume',
  'Outside every allowed volume the command is held, and the margin says by how far.',
  hostParams({ profile: poseProfile({ workspace: [TIGHT] }) }),
  cycle({ shoulder: 0 }), undefined, { frames: POSE_ARM, joints: POSE_ARM_JOINTS })

cycleCase('workspace', 'rotated-out-of-the-slab',
  'A quarter turn takes the tool out of a box that contained it, and the box reports the distance to its face.',
  hostParams({ profile: poseProfile({ workspace: [SLAB] }) }),
  cycle({ shoulder: 1.5707963267948966 }), undefined, { frames: POSE_ARM, joints: POSE_ARM_JOINTS })

cycleCase('workspace', 'layers-compose-by-conjunction',
  'A wide layer cannot re-permit what a narrow one forbade; layering only ever narrows (§38.2).',
  hostParams({ profile: poseProfile({
    workspace: [layer('site', INSIDE.volumes), layer('cell', TIGHT.volumes)],
  }) }),
  cycle({ shoulder: 0 }), undefined, { frames: POSE_ARM, joints: POSE_ARM_JOINTS })

cycleCase('workspace', 'alternatives-within-one-layer',
  'Within a layer the volumes are alternatives — an operator drawing two allowed regions means either.',
  hostParams({ profile: poseProfile({ workspace: [layer('cell', [
    { frame: 'tool', shape: { kind: 'sphere', centre: [5, 0, 0], radius_m: 0.5 } },
    { frame: 'tool', shape: { kind: 'sphere', centre: [1, 0, 0], radius_m: 0.5 } },
  ])] }) }),
  cycle({ shoulder: 0 }), undefined, { frames: POSE_ARM, joints: POSE_ARM_JOINTS })

cycleCase('workspace', 'judged-after-clamping',
  'The pose checked is the one the clamped command produces, not the one the policy asked for.',
  hostParams({
    transport: { ...CLAMP, per_channel: { shoulder: 0 } },
    profile: poseProfile({ workspace: [SLAB] }),
  }),
  cycle({ shoulder: 1.5707963267948966 }), undefined, { frames: POSE_ARM, joints: POSE_ARM_JOINTS })

cycleCase('workspace', 'no-lookahead-on-a-velocity-body',
  'With no position channel bound the guard checks where the body is, which is a weaker claim than §7.1 makes — and the verdict says so.',
  hostParams({ profile: poseProfile({ workspace: [TIGHT] }) }),
  cycle({ shoulder_velocity: 3 }), undefined,
  { frames: POSE_ARM, joints: [{ joint: 'shoulder', channel: null, measured: 0 }] })

/** Two capsules on a shared root, close enough that a margin decides. */
function pairFrames(offset: number, ignore: string[] = []): FrameGraph {
  const capsule = { kind: 'capsule', a: [0, 0, 0] as [number, number, number], b: [0.5, 0, 0] as [number, number, number], radius_m: 0.1 }
  return {
    root: 'base_link',
    convention: 'rep103',
    generator: 'vendor',
    nodes: [
      poseFrame({ name: 'base_link' }),
      poseFrame({
        name: 'left', parent: 'base_link', kind: 'static',
        translation: [0, offset, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
        collision: true, collision_shape: capsule,
        ...(ignore.length > 0 ? { collision_ignore: ignore } : {}),
      }),
      poseFrame({
        name: 'right', parent: 'base_link', kind: 'static',
        translation: [0, -offset, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
        collision: true, collision_shape: capsule,
      }),
    ],
  }
}

cycleCase('collision', 'clear',
  'Two bodies well apart do not trip, and the guard is active rather than silent.',
  hostParams({ profile: poseProfile({ self_collision_margin_m: 0.05 }) }),
  cycle({}), undefined, { frames: pairFrames(0.5) })

cycleCase('collision', 'inside-the-margin',
  'Inside the configured margin the command is held and both frames are named.',
  hostParams({ profile: poseProfile({ self_collision_margin_m: 0.05 }) }),
  cycle({}), undefined, { frames: pairFrames(0.1) })

cycleCase('collision', 'declared-exception',
  'A pair the body declares may touch is skipped, however the exception was written.',
  hostParams({ profile: poseProfile({ self_collision_margin_m: 0.05 }) }),
  cycle({}), undefined, { frames: pairFrames(0.1, ['right']) })

cycleCase('collision', 'no-margin-configured',
  'With no margin the surfaces must actually meet, which is legal and late; the notice is on the limits.',
  hostParams({ profile: poseProfile() }),
  cycle({}), undefined, { frames: pairFrames(0.1) })

limitsCase('pose-guards-active', 'A body with frames, geometry and volumes runs both pose guards.',
  hostParams({ profile: poseProfile({ workspace: [INSIDE], self_collision_margin_m: 0.05 }) }),
  { frames: POSE_ARM })

limitsCase('workspace-without-volumes', 'Frames but no volumes: the guard names the setting it wants.',
  hostParams({ profile: poseProfile() }), { frames: POSE_ARM })

limitsCase('workspace-names-an-absent-frame',
  'A volume naming a frame the body does not have reports that frame, per §44.5.',
  hostParams({ profile: poseProfile({ workspace: [layer('cell', [
    { frame: 'left_hand.tool0', shape: { kind: 'sphere', centre: [0, 0, 0], radius_m: 1 } },
  ])] }) }),
  { frames: POSE_ARM })

limitsCase('one-collision-body', 'One capsule has nothing to collide with, so the guard is unavailable.',
  hostParams({ profile: poseProfile() }), { frames: POSE_ARM })

limitsCase('no-collision-margin', 'The guard runs and the margin is unset, which is a notice of its own.',
  hostParams({ profile: poseProfile() }), { frames: pairFrames(0.5) })

// ── limits ───────────────────────────────────────────────────────────────────
//
// Not one of the six corpora §31.4 lists. Limit resolution is contract behaviour with a
// serialisable output and three composition rules that would otherwise be pinned only indirectly,
// through the cycle vectors that consume it — so it gets its own corpus, per the rule that the
// corpus grows with every feature.

function limitsCase(id: string, note: string, params: ResolveLimitsParams, body: Body = {}): void {
  const frames = body.frames ? flattenOrThrow(body.frames) : null
  add('limits', 'resolution', {
    id, note,
    input: { ...params, ...(body.frames ? { frames: body.frames } : {}) },
    expected: canonical(shapeLimits(resolveLimits({ ...params, frames }))),
  })
}

limitsCase('host-complete', 'Everything declared, every guard available.', hostParams())
limitsCase('guest-advisory', 'The guest tier says it enforces nothing.', hostParams({ tier: 'guest' }))
limitsCase('certified', 'The certified tier has a stop path off the application processor.',
  hostParams({ tier: 'certified' }))
limitsCase('no-envelope', 'Nothing for the kernel to enforce, and it says so.',
  hostParams({ envelope: null, transport: null }))
limitsCase('unsourced', 'A limit without provenance is a rumour.',
  hostParams({ envelope: { ...ENVELOPE, source: undefined } }))
limitsCase('profile-narrows', 'A profile may narrow.',
  hostParams({ profile: { name: 'half', joint_velocity_max_rad_s: 0.5, max_delta_per_cycle: 0.25 } }))
limitsCase('profile-widens', 'A profile may not widen, and is told that it tried.',
  hostParams({ profile: { name: 'faster-please', joint_torque_max_nm: 500, max_delta_per_cycle: 0.25 } }))
limitsCase('profile-widens-all-three', 'Every widened field is named.',
  hostParams({ profile: {
    name: 'greedy', joint_torque_max_nm: 500, joint_velocity_max_rad_s: 9,
    base_velocity_max_m_s: 9, max_delta_per_cycle: 0.25,
  } }))
limitsCase('nothing-available', 'A robot that can supply no guard inputs at all.',
  hostParams({ available: {} }))
limitsCase('wheeled-base', 'Balance on a wheeled base is inapplicable, not broken.',
  hostParams({ available: { ...ALL_AVAILABLE, legged: false } }))
limitsCase('legged-no-imu', 'A legged body missing an IMU is missing an IMU.',
  hostParams({ available: { ...ALL_AVAILABLE, imu: false } }))
limitsCase('proximity-configured', 'Speed and separation monitoring, configured and sensed.',
  hostParams({ profile: {
    name: 'shared-space', max_delta_per_cycle: 0.25,
    proximity: { stop_distance_m: 0.3, slow_distance_m: 1, slow_factor: 0.3 },
  } }))
limitsCase('proximity-unsensed', 'Configured, and nothing detects people.',
  hostParams({
    profile: {
      name: 'shared-space', max_delta_per_cycle: 0.25,
      proximity: { stop_distance_m: 0.3, slow_distance_m: 1, slow_factor: 0.3 },
    },
    available: { ...ALL_AVAILABLE, people_detection: false },
  }))
limitsCase('transport-only', 'Ceilings from the plan with no envelope behind them.',
  hostParams({ envelope: null }))

// ── admission ────────────────────────────────────────────────────────────────

function manifest(overrides: Record<string, unknown> = {}): AdmissionParams['manifest'] {
  return {
    schema_version: 1,
    id: 'acme/grasp',
    version: '1.4.0',
    kind: 'policy',
    digest: 'sha256:pkg',
    isolation: 'sandboxed',
    budget: { ram_mb: 512, vram_mb: 2048, cpu_cores: 2, disk_mb: 4096, inference_budget_ms: 18 },
    embodiment: {
      capabilities: ['manipulation.grasp', 'sensing.rgb'],
      control_mode: 'torque',
    },
    requests: ['manipulation.grasp'],
    ...overrides,
  } as AdmissionParams['manifest']
}

function capacity(overrides: Record<string, unknown> = {}): AdmissionParams['capacity'] {
  return {
    ram_mb: 8192,
    vram_mb: 8192,
    cpu_cores: 6,
    disk_mb: 65536,
    os_version: '1.2.0',
    capabilities: ['manipulation.grasp', 'sensing.rgb'],
    granted: ['manipulation.grasp'],
    installed: [],
    ...overrides,
  } as AdmissionParams['capacity']
}

function admissionCase(
  id: string, note: string, overrides: Partial<AdmissionParams> = {},
): void {
  const params: AdmissionParams = {
    manifest: manifest(),
    capacity: capacity(),
    signature_verified: true,
    observed_digest: 'sha256:pkg',
    ...overrides,
  }
  add('admission', 'install', {
    id, note,
    input: params,
    expected: canonical(shapeAdmission(admitPackage(params))),
  })
}

admissionCase('clean', 'Everything declared, provided and granted.')
admissionCase('unsigned', 'Bytes may be intact and nobody has attested to who produced them.',
  { signature_verified: false })
admissionCase('digest-mismatch', 'The contents do not hash to the manifest digest.',
  { observed_digest: 'sha256:other' })
admissionCase('isolation-violation', 'Only drivers touch hardware directly.',
  { manifest: manifest({ isolation: 'native' }) })
admissionCase('driver-may-be-native', 'A driver is the one kind that may.',
  { manifest: manifest({ kind: 'driver', isolation: 'native' }) })
admissionCase('budget-ram', 'A robot that cannot feed the package refuses it before it runs.',
  { capacity: capacity({ ram_mb: 256 }) })
admissionCase('budget-every-resource', 'Each shortfall is named separately.',
  { capacity: capacity({ ram_mb: 128, vram_mb: 512, cpu_cores: 1, disk_mb: 100 }) })
admissionCase('budget-absent-capacity', 'An undeclared capacity is zero, not unlimited.',
  { capacity: capacity({ vram_mb: undefined, cpu_cores: undefined, disk_mb: undefined }) })
admissionCase('over-request', 'A package cannot request what it never declared a need for.',
  { manifest: manifest({ requests: ['manipulation.grasp', 'locomotion.walk'] }) })
admissionCase('capability-denied', 'The operator profile withholds it outright.',
  { capacity: capacity({ denied: ['manipulation.grasp'] }) })
admissionCase('capability-unprovided', 'The robot does not have it at all.',
  { capacity: capacity({ capabilities: ['sensing.rgb'] }) })
admissionCase('ungranted-is-a-warning', 'The Android runtime-permission moment: it installs and waits.',
  { capacity: capacity({ granted: [] }) })
admissionCase('missing-dependency', 'A declared dependency that is not installed.',
  { manifest: manifest({ depends_on: [{ id: 'acme/vision', range: '^2.0.0' }] }) })
admissionCase('dependency-satisfied', 'The same dependency, present.',
  {
    manifest: manifest({ depends_on: [{ id: 'acme/vision', range: '^2.0.0' }] }),
    capacity: capacity({ installed: [{ id: 'acme/vision', version: '2.1.0' }] }),
  })
admissionCase('os-too-old', 'The robot predates the package minimum.',
  { manifest: manifest({ min_os_version: '2.0.0' }) })
admissionCase('os-exactly-minimum', 'Equal versions satisfy a minimum.',
  { manifest: manifest({ min_os_version: '1.2.0' }) })
admissionCase('transport-refused', 'A policy that cannot reach this body cannot install onto it.',
  { transport_verdict: 'refused' })
admissionCase('transport-degraded-installs', 'Degraded is a decision for the operator, not a refusal.',
  { transport_verdict: 'degraded' })
admissionCase('executes-on-load', 'Loading this package runs code the operator has not accepted.',
  { executes_on_load: true })
admissionCase('executes-on-load-trusted', 'The same package, once the operator has decided.',
  { executes_on_load: true, trust_remote_code: true })
admissionCase('every-finding-at-once',
  'Four problems reported together, because fixing them one deployment at a time is how people end up granting everything.',
  {
    signature_verified: false,
    observed_digest: 'sha256:other',
    manifest: manifest({ isolation: 'native', requests: ['manipulation.grasp', 'locomotion.walk'] }),
    capacity: capacity({ ram_mb: 64 }),
  })

// ── certification ────────────────────────────────────────────────────────────

const GUEST_TESTS = [
  'rhal.abi.version', 'rhal.state.completeness', 'rhal.command.rejection',
  'embodiment.descriptor.valid', 'safety.kernel.parity', 'safety.policy.isolation',
  'package.signature.required', 'package.grants.gate', 'offline.control.independent',
  'security.credential.scope',
]

function results(ids: string[], outcome = 'pass'): CertificationInput['results'] {
  return ids.map(id => ({
    test_id: id,
    outcome: outcome as CertificationInput['results'][number]['outcome'],
    ran_at: '2026-08-29T00:00:00Z',
  }))
}

function certificationCase(id: string, note: string, input: CertificationInput): void {
  add('certification', 'award', {
    id, note,
    input,
    expected: canonical(shapeCertification(awardCertification(input))),
  })
}

certificationCase('no-evidence', 'Absence is not a pass — the loophole every self-certification scheme dies of.',
  { claimed: 'guest', results: [] })
certificationCase('guest-complete', 'Every guest-tier test measured and passing.',
  { claimed: 'guest', results: results(GUEST_TESTS) })
certificationCase('one-skipped', 'A skipped test is missing, not passing.',
  { claimed: 'guest', results: [...results(GUEST_TESTS.slice(1)), ...results([GUEST_TESTS[0]], 'skipped')] })
certificationCase('one-failing', 'A failing test is a failure, and it is named.',
  { claimed: 'guest', results: [...results(GUEST_TESTS.slice(1)), ...results([GUEST_TESTS[0]], 'fail')] })
certificationCase('not-applicable-passes',
  'The balance guard on a wheeled base is genuinely inapplicable; forcing a fail would push vendors to fake a result.',
  { claimed: 'guest', results: [...results(GUEST_TESTS.slice(1)), ...results([GUEST_TESTS[0]], 'not_applicable')] })
certificationCase('downgraded', 'Claimed certified with only guest evidence: downgraded, not refused.',
  { claimed: 'certified', results: results(GUEST_TESTS) })
certificationCase('field-suspends', 'Below the floor the certificate no longer describes the machine.',
  { claimed: 'guest', results: results(GUEST_TESTS), field_conformance: 0.99 })
certificationCase('field-at-floor', 'Exactly at the floor is not below it.',
  { claimed: 'guest', results: results(GUEST_TESTS), field_conformance: 0.999 })
certificationCase('field-tolerates-noise', 'The floor is 0.999 so sensor noise does not suspend an honest vendor.',
  { claimed: 'guest', results: results(GUEST_TESTS), field_conformance: 0.9995 })
certificationCase('bench-lapsed', 'A certificate that cannot expire lies with increasing confidence.',
  { claimed: 'guest', results: results(GUEST_TESTS), bench_age_days: 400 })
certificationCase('bench-fresh', 'Remaining life is reported.',
  { claimed: 'guest', results: results(GUEST_TESTS), bench_age_days: 65 })
certificationCase('bench-exactly-a-year', 'A year old exactly has lapsed.',
  { claimed: 'guest', results: results(GUEST_TESTS), bench_age_days: 365 })
certificationCase('unknown-tier', 'A tier nobody defined awards nothing.',
  { claimed: 'platinum' as CertificationInput['claimed'], results: results(GUEST_TESTS) })

// §35.3 and §70. A simulator declares itself one and the declaration caps the award, so that no
// simulated run is ever presented as bench evidence.
const HOST_TESTS = testsRequiredFor('host').map(test => test.id)

certificationCase('host-on-a-real-body', 'The same evidence, on a body that exists.',
  { claimed: 'host', results: results(HOST_TESTS) })
certificationCase('simulated-caps-at-guest',
  'A simulator declares itself one (§35.3), and the declaration caps the award at guest — while the verdict still records what the evidence would have supported.',
  { claimed: 'host', results: results(HOST_TESTS), simulator: true })
certificationCase('simulated-guest-unchanged',
  'The cap only ever lowers. A guest claim on a simulated body is already at the ceiling and gains no reason.',
  { claimed: 'guest', results: results(GUEST_TESTS), simulator: true })
certificationCase('simulated-no-evidence',
  'The cap never manufactures an award: nothing measured is still nothing awarded, simulated or not.',
  { claimed: 'host', results: [], simulator: true })
certificationCase('simulated-and-lapsed',
  'The cap runs before expiry, so a simulated body with year-old bench results still lapses to none rather than resting at guest.',
  { claimed: 'host', results: results(HOST_TESTS), simulator: true, bench_age_days: 400 })

// ── frames, §44.3 ────────────────────────────────────────────────────────────
//
// The frame graph is pinned here rather than left to whichever generator wrote a file first. Two
// emit it — `skynet descriptor from-urdf` and the MuJoCo driver — and the kernel will read it, so
// a second edition emitting a different spelling of the same tree would be a divergence no test
// catches. These cases are that test.

function frameNode(name: string, parent: string | null, extra: Partial<FrameNode> = {}): FrameNode {
  return {
    name,
    parent,
    kind: parent === null ? 'root' : 'static',
    ...(parent === null ? {} : { translation: [0, 0, 0.1] as [number, number, number],
                                 rotation: [0, 0, 0, 1] as [number, number, number, number],
                                 source: 'descriptor' as const }),
    collision: true,
    ...extra,
  }
}

function graph(nodes: FrameNode[], root = 'base_link'): FrameGraph {
  return { root, convention: 'rep103', generator: 'urdf', nodes }
}

function framesCase(id: string, note: string, input: FrameGraph): void {
  add('frames', 'validate', {
    id, note,
    input: { graph: input },
    expected: canonical(shapeFrameFault(validateFrames(input))),
  })
}

const ARM_FRAMES = graph([
  frameNode('base_link', null),
  frameNode('shoulder', 'base_link', {
    kind: 'dynamic', joint: 'right_arm.shoulder.pitch', joint_type: 'revolute',
    axis: [0, 1, 0], mass_kg: 1.4,
  }),
  frameNode('forearm', 'shoulder', {
    kind: 'dynamic', joint: 'right_arm.elbow.flex', joint_type: 'revolute', axis: [0, 1, 0],
  }),
  frameNode('tool0', 'forearm'),
])

framesCase('well-formed', 'A tree with a root, two dynamic frames and a static tool frame.',
  ARM_FRAMES)
framesCase('empty', 'A body with no frames at all is a claim nobody should be making.',
  graph([]))
framesCase('root-missing', 'The declared root is not one of the frames, so nothing is relative to anything.',
  graph(ARM_FRAMES.nodes, 'torso'))
framesCase('several-roots', 'Two parentless frames is a forest, and forward kinematics over a forest is undefined.',
  graph([...ARM_FRAMES.nodes, frameNode('trolley', null)]))
framesCase('duplicate-name', 'Two frames with one name make every parent reference to it ambiguous.',
  graph([...ARM_FRAMES.nodes, frameNode('forearm', 'base_link')]))
framesCase('parent-missing', 'A parent nobody declared would silently resolve to identity.',
  graph([...ARM_FRAMES.nodes, frameNode('gripper', 'wrist')]))
framesCase('cycle', 'Following parents must terminate; a cycle is a hang in the middle of a control cycle.',
  graph([
    frameNode('base_link', null),
    frameNode('a', 'b'),
    frameNode('b', 'a'),
  ]))
framesCase('dynamic-without-a-joint', 'A dynamic frame that names no joint has no way to move.',
  graph([frameNode('base_link', null), frameNode('shoulder', 'base_link', { kind: 'dynamic' })]))
framesCase('static-with-a-joint', 'A static frame that names one is two statements that disagree.',
  graph([
    frameNode('base_link', null),
    frameNode('shoulder', 'base_link', { joint: 'right_arm.shoulder.pitch' }),
  ]))
framesCase('unknown-kind', 'A kind outside the three is a schema this edition does not implement.',
  graph([frameNode('base_link', null), frameNode('shoulder', 'base_link', { kind: 'floating' as FrameNode['kind'] })]))
framesCase('root-only', 'One frame, no joints. A fixed body with nothing on it is still a valid tree.',
  graph([frameNode('base_link', null)]))

// The collision geometry §44.3 gained so that the `self_collision` guard has something to evaluate.
// Each of these presents far from its cause if it is not caught here: a shape nobody can read looks
// like a limb quietly missing from the collision test, and an exception naming a frame that does
// not exist looks like a pair that trips when the operator believed they had excluded it.

const CAPSULE = { kind: 'capsule', a: [0, 0, 0] as [number, number, number], b: [0, 0, 0.3] as [number, number, number], radius_m: 0.06 }

framesCase('collision-shape-well-formed', 'A capsule on a frame that says it carries geometry.',
  graph([
    frameNode('base_link', null),
    frameNode('upper', 'base_link', { collision_shape: CAPSULE }),
  ]))
framesCase('collision-shape-unknown-kind', 'A shape this edition cannot evaluate is named, never ignored.',
  graph([
    frameNode('base_link', null),
    frameNode('upper', 'base_link', { collision_shape: { ...CAPSULE, kind: 'mesh' } }),
  ]))
framesCase('collision-shape-no-radius', 'A capsule of no radius bounds nothing, and would report clearance it has not established.',
  graph([
    frameNode('base_link', null),
    frameNode('upper', 'base_link', { collision_shape: { ...CAPSULE, radius_m: 0 } }),
  ]))
framesCase('collision-shape-contradicted', 'A shape on a frame that says it carries no geometry is two statements that disagree.',
  graph([
    frameNode('base_link', null),
    frameNode('upper', 'base_link', { collision: false, collision_shape: CAPSULE }),
  ]))
framesCase('collision-ignore-well-formed', 'An exception naming a frame that exists.',
  graph([
    frameNode('base_link', null),
    frameNode('torso', 'base_link', { collision_shape: CAPSULE, collision_ignore: ['pelvis'] }),
    frameNode('pelvis', 'base_link', { collision_shape: CAPSULE }),
  ]))
framesCase('collision-ignore-missing', 'An exception naming nothing excludes nothing, and says so.',
  graph([
    frameNode('base_link', null),
    frameNode('torso', 'base_link', { collision_shape: CAPSULE, collision_ignore: ['pelvis'] }),
  ]))

// ── kinematics ───────────────────────────────────────────────────────────────
//
// Forward kinematics on its own, with the **poses themselves** as the expected value.
//
// A separate corpus from `cycle` because it pins something `cycle` cannot. A trip is a boolean, and
// two editions could agree on every one of them while disagreeing about every pose by a millimetre.
// Thirty digits of translation do not permit that — and they are what hold the two editions' sine,
// cosine and square root to each other, which is the reason those three are written out in
// `crates/skynet-kernel/src/real.rs` and `reference/typescript/kinematics.ts` rather than taken from
// a platform library that rounds its own way.

function kinematicsCase(
  id: string, note: string, frames: FrameGraph,
  joints: JointBinding[], command: Record<string, number>,
): void {
  const flat = flattenOrThrow(frames)
  add('kinematics', 'solve', {
    id, note,
    input: { frames, joints, command: sortedRecord(command) },
    expected: canonical(shapeSolved(flat, solve(flat, joints, command))),
  })
}

/** A two-link planar arm: shoulder a metre up, elbow a metre out, both about z. */
const PLANAR_ARM: FrameGraph = graph([
  poseFrame({ name: 'base_link' }),
  poseFrame({
    name: 'upper', parent: 'base_link', kind: 'dynamic',
    translation: [0, 0, 1], rotation: [0, 0, 0, 1], source: 'descriptor',
    joint: 'shoulder', joint_type: 'revolute', axis: [0, 0, 1],
  }),
  poseFrame({
    name: 'tool', parent: 'upper', kind: 'dynamic',
    translation: [1, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
    joint: 'elbow', joint_type: 'revolute', axis: [0, 0, 1],
  }),
])

const PLANAR_JOINTS: JointBinding[] = [
  { joint: 'shoulder', channel: 'shoulder', measured: 0 },
  { joint: 'elbow', channel: 'elbow', measured: 0 },
]

kinematicsCase('zero-configuration', 'Every joint at zero: the chain lies along its own declared offsets.',
  PLANAR_ARM, PLANAR_JOINTS, { elbow: 0, shoulder: 0 })
kinematicsCase('quarter-turn', 'A quarter turn about z takes the first link from +x to +y, and the second continues along it.',
  PLANAR_ARM, PLANAR_JOINTS, { elbow: 0, shoulder: 1.5707963267948966 })
kinematicsCase('two-joints', 'Both joints turning: the composition is what makes a chain a chain and not a sum of offsets.',
  PLANAR_ARM, PLANAR_JOINTS, { elbow: 0.7853981633974483, shoulder: 0.5235987755982988 })
kinematicsCase('irrational-angles', 'Angles with no exact representation, which is where two libraries would part company.',
  PLANAR_ARM, PLANAR_JOINTS, { elbow: -2.718281828459045, shoulder: 3.141592653589793 })
kinematicsCase('many-revolutions', 'Sixteen revolutions: the range reduction, not the polynomial.',
  PLANAR_ARM, PLANAR_JOINTS, { elbow: -50, shoulder: 100 })
kinematicsCase('past-the-exact-reduction',
  'An angle past the range the specified reduction is exact over leaves the frame unresolved rather than placed wrongly.',
  PLANAR_ARM, PLANAR_JOINTS, { elbow: 0, shoulder: 5e6 })
kinematicsCase('command-does-not-carry-the-joint',
  'A bound channel the command omits falls back to the measured state, and the lookahead flag says the check is the weaker one.',
  PLANAR_ARM, PLANAR_JOINTS, { shoulder: 0.5 })
kinematicsCase('no-binding-at-all',
  'A joint with no binding leaves its frame unresolved, and unresolved propagates down the chain.',
  PLANAR_ARM, [], { elbow: 1, shoulder: 1 })

kinematicsCase('prismatic', 'A prismatic joint slides along its axis rather than turning about it.',
  graph([
    poseFrame({ name: 'base_link' }),
    poseFrame({
      name: 'carriage', parent: 'base_link', kind: 'dynamic',
      translation: [0, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
      joint: 'rail', joint_type: 'prismatic', axis: [0, 0, 1],
    }),
  ]),
  [{ joint: 'rail', channel: 'rail', measured: 0 }], { rail: 0.35 })

kinematicsCase('mimic-with-a-multiplier',
  'A parallel gripper: the second finger runs at −1, and a field that could only say "follows" would put it on the wrong side of the object.',
  graph([
    poseFrame({ name: 'palm' }),
    poseFrame({
      name: 'finger_left', parent: 'palm', kind: 'dynamic',
      translation: [0, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
      joint: 'grip', joint_type: 'prismatic', axis: [0, 1, 0],
    }),
    poseFrame({
      name: 'finger_right', parent: 'palm', kind: 'dynamic',
      translation: [0, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
      joint: 'grip_mirror', joint_type: 'prismatic', axis: [0, 1, 0],
      mimics: 'grip', mimic_multiplier: -1, mimic_offset: 0,
    }),
  ], 'palm'),
  [
    { joint: 'grip', channel: 'grip', measured: 0 },
    { joint: 'grip_mirror', channel: null, measured: 0 },
  ],
  { grip: 0.02 })

kinematicsCase('a-ball-joint-is-unresolved',
  'One scalar cannot say where a ball joint is, so the frame and everything below it report no pose rather than a guessed one.',
  graph([
    poseFrame({ name: 'base_link' }),
    poseFrame({
      name: 'shoulder', parent: 'base_link', kind: 'dynamic',
      translation: [0, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
      joint: 'ball', joint_type: 'ball', axis: [0, 0, 1],
    }),
    poseFrame({
      name: 'tool', parent: 'shoulder', kind: 'static',
      translation: [1, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
    }),
  ]),
  [{ joint: 'ball', channel: 'ball', measured: 0 }], { ball: 0 })

kinematicsCase('a-degenerate-axis-is-a-defect',
  'A zero-length axis is a body-description defect and not a fixed joint; reading it as one would weld a limb in the model while it moved in the world.',
  graph([
    poseFrame({ name: 'base_link' }),
    poseFrame({
      name: 'arm', parent: 'base_link', kind: 'dynamic',
      translation: [0, 0, 0], rotation: [0, 0, 0, 1], source: 'descriptor',
      joint: 'elbow', joint_type: 'revolute', axis: [0, 0, 0],
    }),
  ]),
  [{ joint: 'elbow', channel: 'elbow', measured: 0 }], { elbow: 1 })

kinematicsCase('a-rotated-static-frame',
  'A static rotation composes with everything below it, which is where an xyzw/wxyz mix-up would show.',
  graph([
    poseFrame({ name: 'base_link' }),
    poseFrame({
      name: 'tilted', parent: 'base_link', kind: 'static', source: 'descriptor',
      translation: [0.1, 0.2, 0.3],
      rotation: [0.2705980500730985, 0.2705980500730985, 0.6532814824381883, 0.6532814824381883],
    }),
    poseFrame({
      name: 'tool', parent: 'tilted', kind: 'static', source: 'descriptor',
      translation: [1, 0, 0], rotation: [0, 0, 0, 1],
    }),
  ]),
  [], {})

kinematicsCase('collision-pairs-are-counted',
  'Three capsules on one chain: the quadratic cost is reported rather than assumed small.',
  graph([
    poseFrame({ name: 'base_link' }),
    poseFrame({ name: 'a', parent: 'base_link', kind: 'static', source: 'descriptor', translation: [0, 0, 0.2], rotation: [0, 0, 0, 1], collision: true, collision_shape: CAPSULE }),
    poseFrame({ name: 'b', parent: 'a', kind: 'static', source: 'descriptor', translation: [0, 0, 0.2], rotation: [0, 0, 0, 1], collision: true, collision_shape: CAPSULE }),
    poseFrame({ name: 'c', parent: 'b', kind: 'static', source: 'descriptor', translation: [0, 0, 0.2], rotation: [0, 0, 0, 1], collision: true, collision_shape: CAPSULE }),
  ]),
  [], {})

// ── readiness ────────────────────────────────────────────────────────────────

function subject(overrides: Partial<ReadinessSubject> = {}): ReadinessSubject {
  return {
    slug: 'unitree-g1',
    name: 'Unitree G1',
    dof: 23,
    capabilities: ['safety.estop', 'platform.onboard_compute'],
    control_modes: ['position', 'torque'],
    control_hz: 500,
    compute: { gpu: 'Orin NX' },
    limits: {
      joint_velocity_max_rad_s: 12,
      joint_torque_max_nm: 120,
      watchdog_ms: 50,
      estop: true,
      source: 'https://example.invalid/g1.pdf',
    },
    adapter_count: 2,
    ...overrides,
  }
}

function readinessCase(id: string, note: string, s: ReadinessSubject): void {
  add('readiness', 'robot', {
    id, note,
    input: s,
    expected: canonical(shapeReadiness(evaluateReadiness(s))),
  })
}

readinessCase('fully-declared', 'Every declaration in place: the certified tier may be attempted.', subject())
readinessCase('no-adapter', 'Nothing can talk to this robot yet.', subject({ adapter_count: 0 }))
readinessCase('no-envelope', 'There is nothing for the kernel to enforce.', subject({ limits: null }))
readinessCase('empty-envelope', 'Somebody looked and found nothing — different from nobody looking.',
  subject({ limits: {} }))
readinessCase('no-watchdog', 'Cumulative tiers: this stops the walk at guest.',
  subject({ limits: { ...subject().limits!, watchdog_ms: undefined } }))
readinessCase('no-source', 'A safety limit without provenance is a rumour.',
  subject({ limits: { ...subject().limits!, source: undefined } }))
readinessCase('no-hardware-estop', 'The certified tier needs a stop path off the application processor.',
  subject({ limits: { ...subject().limits!, estop: undefined } }))
readinessCase('no-compute', 'The OS cannot own the control loop without somewhere to run.',
  subject({ capabilities: ['safety.estop'], compute: null }))
readinessCase('no-software-estop', 'A guest-tier requirement.',
  subject({ capabilities: ['platform.onboard_compute'] }))
readinessCase('nothing-declared', 'A registry row with nothing in it.',
  { slug: 'unknown', name: 'Unknown', dof: null, capabilities: [], control_modes: [],
    control_hz: null, compute: null, limits: null, adapter_count: 0 })
readinessCase('one-adapter-singular', 'The sentence agrees with the number.',
  subject({ adapter_count: 1 }))

const FLEET = [
  evaluateReadiness(subject()),
  evaluateReadiness(subject({ slug: 'a', adapter_count: 0 })),
  evaluateReadiness(subject({ slug: 'b', limits: null })),
  evaluateReadiness(subject({ slug: 'c', adapter_count: 0, limits: null })),
]
add('readiness', 'fleet', {
  id: 'mixed-fleet',
  note: 'Where the ecosystem is actually stuck: missing adapters and missing datasheets are different problems.',
  input: {
    subjects: [
      subject(),
      subject({ slug: 'a', adapter_count: 0 }),
      subject({ slug: 'b', limits: null }),
      subject({ slug: 'c', adapter_count: 0, limits: null }),
    ],
  },
  expected: canonical(shapeFleet(summarizeReadiness(FLEET))),
})

// ── fuzz ─────────────────────────────────────────────────────────────────────
//
// §31.4 asks for differential fuzzing in CI: generated inputs run through every edition and
// diffed. The generator is a 32-bit linear congruential PRNG written out in full so that any
// edition can reproduce the same inputs from the same seed, and so that regenerating the corpus
// produces a byte-identical file rather than a reshuffle.

function lcg(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    // Numerical Recipes constants; the arithmetic stays inside 32 bits.
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0
    return state / 4294967296
  }
}

const CHAINS = ['left_arm', 'right_arm', 'torso', 'base', 'aux', 'left_leg']
const AXES = ['pitch', 'yaw', 'roll', 'flex', 'grip']
const KINDS = ['joint_position', 'joint_velocity', 'joint_torque', 'gripper', 'base_velocity', 'discrete']

const rand = lcg(0x5c0ffee5)
const pick = <T>(list: T[]): T => list[Math.floor(rand() * list.length)]

for (let n = 0; n < 60; n += 1) {
  const jointCount = 1 + Math.floor(rand() * 4)
  const joints = []
  for (let j = 0; j < jointCount; j += 1) {
    const role = rand() < 0.85 ? `${pick(CHAINS)}.link${j}.${pick(AXES)}` : null
    joints.push(joint(
      `wire_${j}`,
      role,
      rand() < 0.8 ? Math.round(rand() * 200) : null,
      rand() < 0.5 ? ['torque', 'position'] : ['position'],
    ))
  }
  const target: EmbodimentDescriptor = {
    ...ARM,
    robot_slug: `fuzz-${n}`,
    digest: `sha256:fuzz-${n}`,
    verified: rand() < 0.5,
    base: pick(['fixed', 'wheeled', 'legged']),
    dof: jointCount,
    joints,
    end_effectors: rand() < 0.5 ? ARM.end_effectors : [],
    solvers: rand() < 0.5 ? ['trac_ik'] : [],
    envelope: rand() < 0.6 ? ARM.envelope : null,
  }

  const channelCount = 1 + Math.floor(rand() * 4)
  const channels: ActionChannel[] = []
  for (let c = 0; c < channelCount; c += 1) {
    const role = rand() < 0.8 ? `${pick(CHAINS)}.link${Math.floor(rand() * 4)}.${pick(AXES)}` : null
    channels.push(channel(c, pick(KINDS), role, {
      ...(rand() < 0.5 ? { peak: Math.round(rand() * 300) } : {}),
      ...(rand() < 0.3 ? { optional: true } : {}),
      ...(rand() < 0.2 ? { joint: `wire_${Math.floor(rand() * 4)}` } : {}),
    }))
  }

  const source = policy(channels, {
    dof: 1 + Math.floor(rand() * 6),
    control_mode: pick(['torque', 'position', 'velocity']) as SourceEmbodiment['control_mode'],
    control_hz: Math.round(rand() * 600),
  })
  const options: ResolveTransportOptions = {
    allowRetarget: rand() < 0.5,
    ...(rand() < 0.3 ? { requestedHz: Math.round(rand() * 900) } : {}),
  }

  add('fuzz', 'transport', {
    id: `seeded-${n}`,
    note: 'Generated from seed 0x5c0ffee5. A diff here is a behaviour change, never a reshuffle.',
    input: { source, target, options },
    expected: canonical(shapePlan(resolveTransport(source, target, options))),
  })
}

/**
 * Rebuild a record with its keys in sorted order.
 *
 * This matters more than it looks. The reference iterates `Object.entries(cycle.desired)`, which
 * is insertion order; an edition that parses the same JSON into a sorted map iterates sorted
 * order. Both produce the same `command` object — it gets canonicalised — but `clamped` is an
 * *array*, and its order follows the iteration. So a channel map whose keys were inserted out of
 * order would make two correct implementations disagree about a list.
 *
 * Sorting at generation time removes the disagreement rather than papering over it: a corpus input
 * is canonical JSON, and canonical JSON has sorted keys. Curated cases are written sorted by hand;
 * the fuzz cases go through here.
 */
function sortedRecord(record: Record<string, number>): Record<string, number> {
  const out: Record<string, number> = {}
  for (const key of Object.keys(record).sort()) out[key] = record[key]
  return out
}

const cycleRand = lcg(0xd15ea5e)
for (let n = 0; n < 40; n += 1) {
  const names = ['elbow', 'shoulder', 'wrist', 'base', 'waist']
  const rawDesired: Record<string, number> = {}
  const rawPrevious: Record<string, number> = {}
  const count = 1 + Math.floor(cycleRand() * 4)
  for (let c = 0; c < count; c += 1) {
    const name = names[Math.floor(cycleRand() * names.length)]
    rawDesired[name] = Math.round((cycleRand() * 400 - 200) * 100) / 100
    rawPrevious[name] = Math.round((cycleRand() * 200 - 100) * 100) / 100
  }
  const desired = sortedRecord(rawDesired)
  const previous = sortedRecord(rawPrevious)
  // The profile now sometimes carries separation thresholds, and the cycle sometimes carries a
  // capability token and a distance. Without them the generator produced forty cases in which two
  // of the eight guards could not fire, which is how those two came to be pinned by one edition.
  const wantsProximity = cycleRand() < 0.4
  const params = hostParams({
    tier: (['guest', 'host', 'certified'] as const)[Math.floor(cycleRand() * 3)],
    ...(cycleRand() < 0.3 ? { envelope: null } : {}),
    ...(cycleRand() < 0.3
      ? { profile: null }
      : wantsProximity
        ? {
            profile: {
              name: 'shared-space',
              max_delta_per_cycle: 0.25,
              proximity: { stop_distance_m: 0.3, slow_distance_m: 1, slow_factor: 0.5 },
            },
          }
        : {}),
  })
  const capability = cycleRand() < 0.4
    ? (['motion.arm', 'motion.base', 'gripper'] as const)[Math.floor(cycleRand() * 3)]
    : undefined
  // Staleness is drawn deliberately rather than uniformly. A uniform lag over 300 ms against a
  // 100 ms window made two cases in three a watchdog stop, and a watchdog stop returns before any
  // other guard runs — so most of the fuzz corpus was exercising one guard and reporting coverage
  // of eight.
  const c: CommandCycle = {
    now_ms: 1000,
    command_ms: cycleRand() < 0.2 ? 1000 - Math.round(200 + cycleRand() * 300) : 1000 - Math.round(cycleRand() * 90),
    sequence: 100,
    desired,
    ...(cycleRand() < 0.6 ? { previous } : {}),
    ...(capability === undefined ? {} : { capability }),
    ...(cycleRand() < 0.6 ? { nearest_person_m: Math.round(cycleRand() * 150) / 100 } : {}),
  }
  const lastSequence = cycleRand() < 0.3 ? 100 : 99
  const grants = cycleRand() < 0.5 ? ['motion.arm'] : []

  const limits = withGrants(resolveLimits(params), grants)
  add('fuzz', 'cycle', {
    id: `seeded-${n}`,
    note: 'Generated from seed 0xd15ea5e.',
    input: { limits: { ...params, grants }, cycle: c, last_sequence: lastSequence },
    expected: canonical(shapeVerdict(evaluateCycle(c, limits, params.envelope, lastSequence))),
  })
}

// ── Digests (§37.1) ──────────────────────────────────────────────────────────
//
// The identity a package is verified against. These cases exist because the digest is the one
// contract where a divergence is not a wrong answer but a *different package* — two editions that
// disagreed here would each be certain the other had been tampered with.

const HASH_A = 'a'.repeat(64)
const HASH_B = '0123456789abcdef'.repeat(4)

function file(path: string, sha256 = HASH_A): DigestFile {
  return { path, sha256 }
}

function digestCase(
  id: string,
  note: string,
  input: { kind: string; version: string; manifest: unknown; files: DigestFile[] },
): void {
  add('digest', 'version', {
    id,
    note,
    input,
    expected: canonical(shapeDigest(versionDigest(input))),
  })
}

const POLICY_MANIFEST = {
  schema_version: 1,
  id: 'openrobot/grasp',
  version: '1.4.0',
  kind: 'policy',
  isolation: 'wasm',
  budget: { ram_mb: 512, cpu_cores: 1.5, control_hz: 50 },
}

digestCase(
  'empty',
  'No files and an empty manifest — the degenerate case, pinned so the framing cannot drift unnoticed.',
  { kind: 'policy', version: '0.0.0', manifest: {}, files: [] },
)

digestCase(
  'one-file',
  'One file. The list contributes a single line with no trailing separator.',
  { kind: 'policy', version: '1.0.0', manifest: {}, files: [file('policy.onnx')] },
)

digestCase(
  'manifest-key-order',
  'Keys given in a deliberately unhelpful order. Canonicalisation is what makes the digest a property of the content rather than of the editor that wrote it.',
  {
    kind: 'policy',
    version: '1.4.0',
    manifest: {
      isolation: 'wasm',
      budget: { control_hz: 50, cpu_cores: 1.5, ram_mb: 512 },
      kind: 'policy',
      version: '1.4.0',
      id: 'openrobot/grasp',
      schema_version: 1,
    },
    files: [file('policy.onnx', HASH_B), file('manifest.json')],
  },
)

digestCase(
  'manifest-canonical-equivalent',
  'The same manifest written the natural way, and the same two files. Must produce the identical digest to `manifest-key-order`, which is the whole claim.',
  {
    kind: 'policy',
    version: '1.4.0',
    manifest: POLICY_MANIFEST,
    files: [file('manifest.json'), file('policy.onnx', HASH_B)],
  },
)

digestCase(
  'kind-separates',
  'Same manifest and files, different repository kind. A dataset and a policy holding identical bytes are not the same artifact.',
  {
    kind: 'dataset',
    version: '1.4.0',
    manifest: POLICY_MANIFEST,
    files: [file('manifest.json'), file('policy.onnx', HASH_B)],
  },
)

digestCase(
  'nested-paths',
  'Nested paths, ordered by UTF-16 code unit — byte order for every path anybody has, pinned here so an edition that reached for its own collation is caught.',
  {
    kind: 'policy',
    version: '2.0.0',
    manifest: POLICY_MANIFEST,
    files: [file('weights/b.bin'), file('weights/a.bin', HASH_B), file('README.md'), file('weights.bin')],
  },
)

digestCase(
  'null-is-not-absent',
  'A null field is present and null. Collapsing it to absent would give two different manifests one identity.',
  { kind: 'policy', version: '1.0.0', manifest: { envelope: null }, files: [] },
)

digestCase(
  'traversal-refused',
  'A path that leaves the package root. Refused before hashing, because a digest computed over a traversal is a digest that attests to one.',
  { kind: 'policy', version: '1.0.0', manifest: {}, files: [file('../../etc/systemd/system/evil.service')] },
)

digestCase('absolute-refused', 'An absolute path.', {
  kind: 'policy',
  version: '1.0.0',
  manifest: {},
  files: [file('/etc/passwd')],
})

digestCase(
  'backslash-refused',
  'A backslash: a separator on one filesystem and an ordinary filename character on another.',
  { kind: 'policy', version: '1.0.0', manifest: {}, files: [file('weights\\a.bin')] },
)

digestCase(
  'trailing-dot-refused',
  'A component ending in a dot, which some filesystems silently strip — so two paths would name one file after unpacking.',
  { kind: 'policy', version: '1.0.0', manifest: {}, files: [file('weights./a.bin')] },
)

digestCase(
  'duplicate-path-refused',
  'Two entries for one path. Which bytes the digest attests to would depend on a sort being stable.',
  { kind: 'policy', version: '1.0.0', manifest: {}, files: [file('a.bin', HASH_A), file('a.bin', HASH_B)] },
)

digestCase(
  'uppercase-hash-refused',
  'An uppercase hash. One digest with two spellings makes every string comparison downstream a coin toss.',
  { kind: 'policy', version: '1.0.0', manifest: {}, files: [file('a.bin', 'A'.repeat(64))] },
)

digestCase('short-hash-refused', 'A truncated hash.', {
  kind: 'policy',
  version: '1.0.0',
  manifest: {},
  files: [file('a.bin', 'abc')],
})

digestCase(
  'huge-number-refused',
  'A manifest number at 1e21, where the Hub reaches for an exponent and the canonical rule forbids one. Refused rather than digested into an identity only one side reproduces.',
  { kind: 'dataset', version: '1.0.0', manifest: { bytes: 1e21 }, files: [] },
)

digestCase('tiny-number-refused', 'The same divergence at the other end: below 1e-6.', {
  kind: 'policy',
  version: '1.0.0',
  manifest: { tolerance: 1e-7 },
  files: [],
})

digestCase(
  'boundary-numbers-accepted',
  'Both boundaries from the inside. 1e-6 and a shade under 1e21 are spelled identically by both, so they are digested rather than refused.',
  { kind: 'policy', version: '1.0.0', manifest: { tolerance: 1e-6, bytes: 999999999999999900000 }, files: [] },
)

digestCase(
  'astral-key-refused',
  'An object key above U+FFFF. UTF-16 order and UTF-8 order disagree there, so the two editions would sort the manifest differently.',
  { kind: 'policy', version: '1.0.0', manifest: { '\u{1F680}': 'launch' }, files: [] },
)

digestCase(
  'astral-value-accepted',
  'The same character in a value. Nothing sorts on a value, so there is nothing to disagree about.',
  { kind: 'policy', version: '1.0.0', manifest: { note: '\u{1F680}' }, files: [] },
)

// ── The black box (§13, §26) ─────────────────────────────────────────────────
//
// Three corpora, and they hold three different kinds of thing.
//
// `schemas` pins the six MCAP channel schemas byte for byte. §31.2 puts them in the short list of
// artifacts that stay single-source however many editions exist, because *a black box sealed by one
// edition must be readable by every other*, and the only way to hold that is to freeze the bytes.
//
// `records` pins the ring's record layout. It is the format's hot path and the part with no
// external specification to fall back on: get an offset wrong and a recording deserialises into
// plausible nonsense rather than failing.
//
// `header` pins the ring header, including both of its tables.
//
// **Every 64-bit field is a decimal string in these inputs.** A wall-clock epoch in nanoseconds is
// about 1.7e18, which is past 2^53, and a corpus whose inputs went through a JSON number would be
// comparing two editions on a value neither of them received intact.

function blackboxCase(file: string, id: string, note: string, input: unknown, expected: string): void {
  add('blackbox', file, { id, note, input, expected })
}

for (const channel of wire()) {
  blackboxCase(
    'schemas',
    channel.name,
    `The embedded schema for ${channel.topic}. §31.2 keeps these single-source across editions: a black box sealed by one must be readable by every other.`,
    { channel: channel.topic },
    canonical({ id: channel.id, topic: channel.topic, name: channel.name, definition: channel.definition }),
  )
}

const BB_SHAPE: RingShape = { joints: 2, channels: 2 }
const BB_OBSERVATION = new Uint8Array(32).fill(0xab)

function bbCycle(overrides: Partial<Cycle> = {}): Cycle {
  return {
    tick: 7n,
    observation: BB_OBSERVATION,
    policy: 0,
    stateFlags: 1,
    faultCode: 0,
    rhalResult: 0,
    stateSeq: 3,
    stateStampNs: 100n,
    commandStampNs: 90n,
    commandSeq: 4,
    mode: 0,
    advisory: false,
    response: null,
    tripped: [],
    capability: 0xffff,
    nearestPersonM: Number.NaN,
    basePose: [Number.NaN, Number.NaN, Number.NaN, Number.NaN, Number.NaN, Number.NaN, Number.NaN],
    baseTwist: [0, 0, 0, 0, 0, 0],
    desired: [0.25, -0.5],
    command: [0.25, -0.5],
    mask: [1, 0],
    clamped: [false, false],
    position: [0.1, 0.2],
    velocity: [0.0, Number.NaN],
    effort: [1.5, Number.NaN],
    ...overrides,
  }
}

/** The JSON shape a corpus input uses for one cycle. 64-bit fields are decimal strings. */
function cycleInput(cycle: Cycle): unknown {
  const orNull = (value: number) => (Number.isFinite(value) ? value : null)
  return {
    kind: 'cycle',
    tick: String(cycle.tick),
    observation: hex(cycle.observation),
    policy: cycle.policy,
    state_flags: cycle.stateFlags,
    fault_code: cycle.faultCode,
    rhal_result: cycle.rhalResult,
    state_seq: cycle.stateSeq,
    state_stamp_ns: String(cycle.stateStampNs),
    command_stamp_ns: String(cycle.commandStampNs),
    command_seq: cycle.commandSeq,
    mode: cycle.mode,
    advisory: cycle.advisory,
    response: cycle.response,
    tripped: cycle.tripped,
    capability: cycle.capability,
    nearest_person_m: orNull(cycle.nearestPersonM),
    base_pose: cycle.basePose.map(orNull),
    base_twist: cycle.baseTwist.map(orNull),
    desired: cycle.desired,
    command: cycle.command,
    mask: cycle.mask,
    clamped: cycle.clamped,
    position: cycle.position.map(orNull),
    velocity: cycle.velocity.map(orNull),
    effort: cycle.effort.map(orNull),
  }
}

function recordCase(
  id: string, note: string, shape: RingShape, sequence: bigint, monoNs: bigint,
  entry: BlackboxEntry, entryInput: unknown,
): void {
  const bytes = encodeRecord(shape, sequence, monoNs, entry)
  blackboxCase('records', id, note,
    { shape, sequence: String(sequence), mono_ns: String(monoNs), entry: entryInput },
    canonical({ size: bytes.length, hex: hex(bytes) }))
}

recordCase(
  'cycle-quiet',
  'A cycle nothing tripped on. Unmeasured velocity and effort are NaN in the ring, which is not zero (§24), and the hold mask says nothing rather than saying zero on the second channel.',
  BB_SHAPE, 5n, 1234n,
  { kind: 'cycle', cycle: bbCycle() }, cycleInput(bbCycle()),
)

{
  const clamped = bbCycle({
    desired: [2.5, -0.5],
    command: [1.0, -0.5],
    clamped: [true, false],
    tripped: ['envelope'],
    response: 'clamp',
    rhalResult: -110,
    mode: 1,
  })
  recordCase(
    'cycle-clamped',
    'A clamp. `desired` is f32 because it came through the RHAL; `command` is f64 because the kernel computed it, and rounding it would make every clamped cycle report a false divergence on replay.',
    BB_SHAPE, 6n, 2234n,
    { kind: 'cycle', cycle: clamped }, cycleInput(clamped),
  )
}

{
  const stopped = bbCycle({
    desired: [2.5, 2.5],
    command: [0, 0],
    tripped: ['watchdog'],
    response: 'stop_cat2',
    commandSeq: 4,
  })
  recordCase(
    'cycle-stopped',
    'A watchdog stop: the kernel sends nothing, so the command array is the zeroes a fixed-size slot already holds. The tripped guard is what distinguishes this from a cycle that asked for zero.',
    BB_SHAPE, 7n, 3234n,
    { kind: 'cycle', cycle: stopped }, cycleInput(stopped),
  )
}

recordCase(
  'frame-two-streams',
  'An assembled observation frame. The worst skew and the per-stream ages are what make "the frame it was given was 80 ms skewed" checkable months later (§25.1).',
  BB_SHAPE, 8n, 4234n,
  {
    kind: 'frame',
    frame: {
      observation: BB_OBSERVATION,
      targetNs: 4230n,
      worstSkewUs: 31000,
      streamsOmitted: 0,
      streams: [
        { name: 'wrist_cam', ageUs: 31000, stale: true },
        { name: 'joints', ageUs: 900, stale: false },
      ],
    },
  },
  {
    kind: 'frame',
    observation: hex(BB_OBSERVATION),
    target_ns: '4230',
    worst_skew_us: 31000,
    streams_omitted: 0,
    streams: [
      { name: 'wrist_cam', age_us: 31000, stale: true },
      { name: 'joints', age_us: 900, stale: false },
    ],
  },
)

recordCase(
  'frame-overflowing',
  'Eight streams into a record that carries six. The two it cannot hold are counted, because a record that silently dropped them would be partial without saying so.',
  BB_SHAPE, 9n, 5234n,
  {
    kind: 'frame',
    frame: {
      observation: BB_OBSERVATION,
      targetNs: 5230n,
      worstSkewUs: 12,
      streamsOmitted: 0,
      streams: ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'].map(name => ({ name, ageUs: 10, stale: false })),
    },
  },
  {
    kind: 'frame',
    observation: hex(BB_OBSERVATION),
    target_ns: '5230',
    worst_skew_us: 12,
    streams_omitted: 0,
    streams: ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'].map(name => ({ name, age_us: 10, stale: false })),
  },
)

recordCase(
  'frame-astral-stream-name',
  'A stream name that does not fit, cut on a character boundary. A cut inside a multi-byte sequence would put invalid UTF-8 in the ring and every reader would have to guess.',
  BB_SHAPE, 10n, 6234n,
  {
    kind: 'frame',
    frame: {
      observation: BB_OBSERVATION,
      targetNs: 6230n,
      worstSkewUs: 0,
      streamsOmitted: 0,
      streams: [{ name: 'ααααααααα', ageUs: 1, stale: false }],
    },
  },
  {
    kind: 'frame',
    observation: hex(BB_OBSERVATION),
    target_ns: '6230',
    worst_skew_us: 0,
    streams_omitted: 0,
    streams: [{ name: 'ααααααααα', age_us: 1, stale: false }],
  },
)

recordCase(
  'event-estop',
  'An estop, on the channel §26 gives lease grants, pre-emptions, OTA and grant changes. The vocabulary is closed, so ordinary logging cannot reach the black box (§41.1).',
  BB_SHAPE, 11n, 7234n,
  { kind: 'event', event: { code: 'estop', subject: 'operator', detail: 'the button on the chassis' } },
  { kind: 'event', code: 'estop', subject: 'operator', detail: 'the button on the chassis' },
)

recordCase(
  'gap-encoder-behind',
  'A gap annotated as a gap (§47.4). It consumes a sequence number of its own, so the numbering stays dense and a reader can tell "the ring wrapped past these" from "these were never written".',
  BB_SHAPE, 12n, 8234n,
  { kind: 'gap', gap: { dropped: 150n, firstTick: 900n, reason: 'encoder_behind' } },
  { kind: 'gap', dropped: '150', first_tick: '900', reason: 'encoder_behind' },
)

const BB_GEOMETRY: RingGeometry = { shape: BB_SHAPE, capacity: 8, digests: 2, capabilities: 2 }
const BB_META: RingMeta = {
  robotId: 'dev-0001',
  embodiment: new Uint8Array(32).fill(0x11),
  osImage: new Uint8Array(32).fill(0x22),
  tier: 1,
  enforcement: 1,
  simulator: false,
  wallEpochNs: 1700000000000000000n,
  monoNs: 5000000000n,
  channels: ['right_arm.shoulder.pitch', 'right_arm.elbow.pitch'],
}

function metaInput(meta: RingMeta): unknown {
  return {
    robot_id: meta.robotId,
    embodiment: hex(meta.embodiment),
    os_image: hex(meta.osImage),
    tier: meta.tier,
    enforcement: meta.enforcement,
    simulator: meta.simulator,
    wall_epoch_ns: String(meta.wallEpochNs),
    mono_ns: String(meta.monoNs),
    channels: meta.channels,
  }
}

function headerCase(
  id: string, note: string, geometry: RingGeometry, meta: RingMeta,
  digests: Uint8Array[], capabilities: string[],
): void {
  const bytes = encodeHeader(geometry, meta, digests, capabilities)
  blackboxCase('header', id, note, {
    geometry: {
      shape: geometry.shape,
      capacity: geometry.capacity,
      digests: geometry.digests,
      capabilities: geometry.capabilities,
    },
    meta: metaInput(meta),
    digests: digests.map(hex),
    capabilities,
  }, canonical({ size: bytes.length, record_size: recordSize(geometry.shape), hex: hex(bytes) }))
}

headerCase(
  'empty-tables',
  'A freshly created ring. The tables are zero, which is how a reader knows where they end, and the header carries its own CRC because damaged metadata makes every record below it unreadable.',
  BB_GEOMETRY, BB_META, [], [],
)

headerCase(
  'filled-tables',
  'One policy digest and one capability token interned. Both live in the header rather than in a record, because a record is overwritten on the next wrap and the recording would then know it had a policy without knowing which.',
  BB_GEOMETRY, BB_META, [new Uint8Array(32).fill(0x33)], ['manipulation.grasp'],
)

// One whole sealed file, which is the artifact §26 is actually about.
//
// The expected value is the file's length, its SHA-256 and its sequence of record opcodes, rather
// than its hex. Every *component* is already pinned exactly — the schemas by the `schemas` corpus,
// the records by `records`, the header by `header` — so what is left for this case to hold is the
// framing and the order. The digest holds them; the opcode list says *where* a divergence is,
// which a digest cannot, and it is the string that made the first one — schemas interleaved with
// channels in one edition and grouped in the other — legible in a second.
{
  const channels = wire()
  const messages = [
    { channelId: 6, logTime: 1700000000000001000n, payload: canonical({ mono_ns: 1000, event: 'recording_started', subject: 'dev-0001', detail: 'the ring was opened' }) },
    { channelId: 1, logTime: 1700000000000002000n, payload: canonical({ mono_ns: 2000, stamp_ns: 100, seq: 3, flags: 1, fault_code: 0, position_rad: [0.1, 0.2], velocity_rad_s: [0, null], effort_nm: [1.5, null], base_pose: [null, null, null, null, null, null, null], base_twist: [0, 0, 0, 0, 0, 0] }) },
  ]
  const bytes = writeMcap(
    'skynet-blackbox rust-edition 0.1.0',
    channels.map(channel => ({ id: channel.id, name: channel.name, definition: channel.definition })),
    channels.map(channel => ({ id: channel.id, schemaId: channel.id, topic: channel.topic })),
    [{ name: 'skynet.blackbox', pairs: [['robot_id', 'dev-0001'], ['sealed_reason', 'operator']] }],
    messages,
  )
  blackboxCase('seal', 'two-messages', 'One sealed file: magic, header, six schemas, six channels, one metadata record, two messages, data end, the summary repeated, three summary offsets, footer, magic. The digest is what holds the framing; the bytes of every component are pinned by the other three corpora.',
    {
      library: 'skynet-blackbox rust-edition 0.1.0',
      metadata: { name: 'skynet.blackbox', pairs: [['robot_id', 'dev-0001'], ['sealed_reason', 'operator']] },
      messages: messages.map(message => ({
        channel_id: message.channelId,
        log_time: String(message.logTime),
        payload: message.payload,
      })),
    },
    canonical({ size: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'), opcodes: opcodes(bytes) }))
}

// ── Write ────────────────────────────────────────────────────────────────────

let cases = 0
for (const [key, list] of [...files.entries()].sort()) {
  const path = join(CORPUS, `${key}.json`)
  mkdirSync(dirname(path), { recursive: true })
  const [corpus] = key.split('/')
  const document = {
    corpus,
    generated_by: 'reference/typescript, via tools/corpus-gen/generate.ts',
    cases: list,
  }
  writeFileSync(path, `${JSON.stringify(document, null, 2)}\n`, 'utf8')
  cases += list.length
  console.log(`${key.padEnd(28)} ${String(list.length).padStart(4)} cases`)
}
console.log(`\n${files.size} files, ${cases} cases`)
