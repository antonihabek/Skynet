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

import { canonical } from '../../reference/typescript/canonical.ts'
import {
  parseRole, resolveTransport, roleKey, validateFrames,
  type ActionChannel, type EmbodimentDescriptor, type FrameGraph, type FrameNode,
  type ResolveTransportOptions, type SourceEmbodiment,
} from '../../reference/typescript/embodiment.ts'
import { admitPackage, type AdmissionParams } from '../../reference/typescript/package.ts'
import {
  awardCertification, testsRequiredFor, type CertificationInput,
} from '../../reference/typescript/conformance.ts'
import {
  evaluateReadiness, summarizeReadiness, type ReadinessSubject,
} from '../../reference/typescript/readiness.ts'
import {
  evaluateCycle, resolveLimits,
  type CommandCycle, type ResolveLimitsParams,
} from '../../reference/typescript/safety.ts'
import type { SafetyEnvelope } from '../../reference/typescript/types.ts'
import {
  shapeAdmission, shapeCertification, shapeFleet, shapeFrameFault, shapeLimits, shapePlan,
  shapeReadiness, shapeVerdict,
} from '../../reference/typescript/shape.ts'

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

function cycleCase(
  file: string, id: string, note: string,
  limitsParams: ResolveLimitsParams, cycle: CommandCycle,
  lastSequence?: number,
): void {
  const limits = resolveLimits(limitsParams)
  const verdict = evaluateCycle(cycle, limits, limitsParams.envelope, lastSequence)
  add('cycle', file, {
    id,
    note,
    input: {
      limits: limitsParams,
      cycle,
      last_sequence: lastSequence === undefined ? null : lastSequence,
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

const ALL_AVAILABLE = {
  imu: true, people_detection: true, collision_model: true, solvers: true, legged: true,
}

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

// ── limits ───────────────────────────────────────────────────────────────────
//
// Not one of the six corpora §31.4 lists. Limit resolution is contract behaviour with a
// serialisable output and three composition rules that would otherwise be pinned only indirectly,
// through the cycle vectors that consume it — so it gets its own corpus, per the rule that the
// corpus grows with every feature.

function limitsCase(id: string, note: string, params: ResolveLimitsParams): void {
  add('limits', 'resolution', {
    id, note,
    input: params,
    expected: canonical(shapeLimits(resolveLimits(params))),
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
  const params = hostParams({
    tier: (['guest', 'host', 'certified'] as const)[Math.floor(cycleRand() * 3)],
    ...(cycleRand() < 0.3 ? { envelope: null } : {}),
    ...(cycleRand() < 0.3 ? { profile: null } : {}),
  })
  const c: CommandCycle = {
    now_ms: 1000,
    command_ms: 1000 - Math.round(cycleRand() * 300),
    sequence: 100,
    desired,
    ...(cycleRand() < 0.6 ? { previous } : {}),
  }
  const lastSequence = cycleRand() < 0.3 ? 100 : 99

  const limits = resolveLimits(params)
  add('fuzz', 'cycle', {
    id: `seeded-${n}`,
    note: 'Generated from seed 0xd15ea5e.',
    input: { limits: params, cycle: c, last_sequence: lastSequence },
    expected: canonical(shapeVerdict(evaluateCycle(c, limits, params.envelope, lastSequence))),
  })
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
