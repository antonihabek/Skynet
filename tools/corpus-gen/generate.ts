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
  ACTION_KINDS, BASE_KINDS, CANONICAL_AXES, CANONICAL_CHAINS, CONTROL_MODES,
  EMBODIMENT_MODALITIES, JOINT_TYPES, parseRole, resolveTransport, roleKey,
  TRANSPORT_METHODS, TRANSPORT_VERDICTS, TRANSPORT_WARNING_CODES,
  validateFrameBindings, validateFrames,
  type ActionChannel, type EmbodimentDescriptor, type FrameGraph, type FrameNode,
  type ResolveTransportOptions, type SourceEmbodiment,
} from '../../reference/typescript/embodiment.ts'
import {
  admitPackage, ISOLATION_MODES, PACKAGE_KINDS, PACKAGE_REJECTION_CODES,
  RESERVED_CAPABILITIES, SIGNATURE_STATES, type AdmissionParams,
} from '../../reference/typescript/package.ts'
import { versionDigest, type DigestFile } from '../../reference/typescript/digest.ts'
import {
  awardCertification, SCTS_AREAS, SCTS_SUITES, testsRequiredFor, type CertificationInput,
} from '../../reference/typescript/conformance.ts'
import {
  evaluateReadiness, summarizeReadiness, type ReadinessSubject,
} from '../../reference/typescript/readiness.ts'
import {
  allSets, evaluate as evaluateCalibration, guardsUnavailable, setsFor,
  type CalibrationRecord, type Conditions, type Validity,
} from '../../reference/typescript/calibration.ts'
import type { Windows as WatchdogWindows } from '../../reference/typescript/watchdog.ts'
import {
  NEVER_SHED, POWER_FAULT_CODES, POWER_REASONS, POWER_STATES, RESERVE_TERMS, SHED_ORDER,
  THERMAL_PRESSURES,
  type PowerObservation, type PowerProfile,
} from '../../reference/typescript/power.ts'
import {
  assembleObservation, latestCommonInstant, type Stream as ObservationStream,
} from '../../reference/typescript/observation.ts'
import {
  ENFORCEMENT_MODES, evaluateCycle, FAULT_RESPONSES, GUARDS, OS_TIERS, resolveLimits, withGrants,
  type CommandCycle, type ResolveLimitsParams,
} from '../../reference/typescript/safety.ts'
import type { SafetyEnvelope } from '../../reference/typescript/types.ts'
import {
  shapeAdmission, shapeCertification, shapeFleet, shapeFrameFault, shapeLimits, shapePlan,
  shapeDigest, shapeObservation, shapeReadiness, shapeSolved, shapeVerdict, shapeCalibration,
  shapeWatchdog, shapePower, shapePredicate, shapeSatisfies, shapeVersion,
  shapeComposition, shapeDeadline, shapePreconditions, shapeSuccess,
  shapeCellFaults, shapeCellLayer,
  shapeStateProperties, shapeTransition, shapeWarningMatrix, shapeSkillTransition,
  shapeLiveContext,
  shapeBundleVerdict, shapeBootVerdict,
  shapeAttestationReport, shapeAttested, shapeCodeResult, shapeIdentityVerdict,
  shapeProvenance, shapeScores,
  shapeCheckIn, shapeClaimVerdict, shapeHealthReport,
  shapeCoverage, shapeOfferVerdict, shapeWindowReport,
} from '../../reference/typescript/shape.ts'
import {
  CELL_FAULTS, cellEnvironment, cellWorkspaceLayer, validateCell, type CellDescriptor,
} from '../../reference/typescript/cell.ts'
import { liveContext, type LiveInputs } from '../../reference/typescript/live.ts'
import {
  admitBundle, BOOT_DECISIONS, BUNDLE_FAULTS, CERTIFICATION_GATED, confirmBoot, OS_COMPONENTS,
  type BootEvidence, type BundleManifest, type UpdateSubject,
} from '../../reference/typescript/ota.ts'
import {
  attest, checkIdentity, CODE_FAULTS, DEVICE_SCOPES, IDENTITY_FINDINGS, KEY_BINDINGS,
  MEASURE_STATES, normalizePairingCode,
  type Attestation, type Identity, type Measure, type Observed,
} from '../../reference/typescript/identity.ts'
import {
  PROVENANCE_CLASSES, PROVENANCE_REASON_CODES, classify,
  type RunBundle,
} from '../../reference/typescript/provenance.ts'
import {
  BIRTH_FINDINGS, check as checkBirth, unsigned as unsignedCertificate,
  OWNERSHIP_FIELDS as BIRTH_OWNERSHIP_FIELDS, REQUIRED_FIELDS as BIRTH_REQUIRED_FIELDS,
  type BirthCertificate, type Observed as BirthObserved,
} from '../../reference/typescript/birth.ts'
import {
  checkIn, claimFor, CLAIMS, HEALTH_FIELDS, READINESS, STAGES, summarize,
  type CheckInInput, type HealthInput, type Progress,
} from '../../reference/typescript/checkin.ts'
import {
  TELEMETRY_COUNTERS, WINDOW_FAULTS, assessWindow, coverage,
  type Window as TelemetryWindow,
} from '../../reference/typescript/telemetry.ts'
import {
  assessOffer, OFFER_FAULTS,
  type Held, type Offer,
} from '../../reference/typescript/intent.ts'
import {
  evaluate as evaluateState, HOMING_STATES, KERNEL_STATES, OPERATING_STATES, POLICY_STATES,
  POST_VERDICTS, RHAL_STATES, STATE_REASONS, STOP_CAUSES,
  type Conditions as OpConditions, type OperatingState,
} from '../../reference/typescript/opstate.ts'
import type { Volume } from '../../reference/typescript/kinematics.ts'
import {
  advance as advanceSkill, boundedDeadline, CLAUSE_ORIGINS, CLAUSE_OUTCOMES, COMPOSITION_FAULTS,
  DEFAULT_OVERRUN_FACTOR, evaluatePreconditions, judgeSuccess, LEASE_STATES, LIFECYCLE_REASONS,
  PHASES, readCalls, readClauses, resolveClauses, skillDeadline, SKILL_OUTCOMES, SKILL_PHASES,
  validateComposition, type Phase, type Progress as SkillProgress, type SkillPhase,
} from '../../reference/typescript/skill.ts'
import {
  evaluatePredicate, evaluatePredicateOutcome, OUTCOMES, PREDICATE_OPS, UNEVALUABLE_REASONS,
} from '../../reference/typescript/predicate.ts'
import {
  compareVersions, isValidRange, isValidVersion, parseVersion, satisfies,
} from '../../reference/typescript/semver.ts'
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

type JointNode = EmbodimentDescriptor['joints'][number]

function joint(
  name: string, role: string | null, effort: number | null, modes: JointNode['control_modes'],
): JointNode {
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
    signature: 'verified',
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
// The four signature states that are findings, each its own case.
//
// They were one case and one boolean until the twelfth session, and the collapse mattered: an
// operator reading `unsigned` could not tell whether to chase a publisher, upgrade the OS, or stop
// and investigate. `untrusted` joined them the moment a verifier existed to discover it — a
// signature can be entirely valid and made by a stranger, and the remedy for that is in the trust
// store rather than anywhere near the package. Four cases because there are four different jobs at
// the end of them.
admissionCase('signature-absent', 'Nobody signed it. The bytes may be intact and nobody has attested to who produced them.',
  { signature: 'absent' })
admissionCase('signature-failed', 'A signature was present and did not verify. The one state that says something went wrong rather than something is missing.',
  { signature: 'failed' })
admissionCase('signature-untrusted', 'It verified, against a key nobody here has trusted. Nothing is wrong with the bytes; what is missing is a decision.',
  { signature: 'untrusted' })
admissionCase('signature-unverifiable', 'The platform cannot check. A gap in the operating system rather than in the package, and the remedy belongs on a different desk.',
  { signature: 'unverifiable' })
admissionCase('digest-mismatch', 'The contents do not hash to the manifest digest.',
  { observed_digest: 'sha256:other' })

// §55.1. An environment is a simulation artifact, and the OS says so rather than accepting one
// silently onto a robot that can never load it.
admissionCase('environment-on-a-physical-robot',
  'An environment describes a simulated world — simulators, a scene, assets, physics and render '
  + 'rates — and only a simulator RHAL driver can load one. A warning rather than an error: '
  + 'installing it is not dangerous, and refusing it would stop an operator staging a package onto '
  + 'a robot they are about to point at a simulator. What is not acceptable is silence.',
  { manifest: manifest({ kind: 'environment' }) })

admissionCase('environment-on-a-simulator',
  'The same package where it belongs. The flag comes from the descriptor, where §35.3 puts the '
  + 'obligation to declare.',
  { manifest: manifest({ kind: 'environment' }), capacity: capacity({ simulator: true }) })

admissionCase('environment-needing-a-capability-the-sim-does-not-have',
  '§55.1: "its `required_capabilities` are still checked at admission, since an environment that '
  + 'needs a gripper is not runnable by a robot without one in sim either". Two findings, and the '
  + 'capability one is the error.',
  {
    manifest: manifest({
      kind: 'environment',
      embodiment: { capabilities: ['manipulation.grasp', 'locomotion.walk'], control_mode: 'torque' },
    }),
    capacity: capacity({ simulator: true }),
  })

admissionCase('a-policy-on-a-simulator-is-just-a-policy',
  'The flag changes nothing for any other kind. A simulator is a robot as far as admission is '
  + 'concerned, which is the whole point of §35.3 — the thing being tested has to be the thing '
  + 'that ships.',
  { capacity: capacity({ simulator: true }) })
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
// The Hub's spelling of a package's requirements, arriving on a document whose embodiment block
// does not carry them. `manifest.json` on a robot *is* the Hub's `manifest` column, and §8.2
// computes admission from `embodiment.capabilities` — so the first of these is a package that would
// otherwise install with a completely clean report, having been checked against nothing.
admissionCase('requirements-the-embodiment-block-does-not-carry',
  'The card fields declare what the package needs and the embodiment block is empty, so '
  + 'nothing enforced them. The worst shape this can take: a green install of an unchecked package.',
  {
    manifest: manifest({
      embodiment: { capabilities: [], control_mode: 'torque' },
      requests: [],
      required_capabilities: ['manipulation.grasp'],
      required_extensions: ['unitree.raw_lowlevel'],
    }),
  })
admissionCase('requirements-declared-in-both-places-agree',
  'The two spellings carry the same tokens, which is the shape a publisher who filled in both '
  + 'should get, and it is not a finding.',
  {
    manifest: manifest({
      required_capabilities: ['manipulation.grasp', 'sensing.rgb'],
    }),
  })
admissionCase('an-embodiment-block-stricter-than-the-card-is-not-a-finding',
  'Declaring more in `embodiment` than on the card is a Hub that under-reports what the '
  + 'package needs, not a robot that under-enforces them. The subset test runs one way for that '
  + 'reason: refusing here would refuse the package whose enforced declaration is the stricter one.',
  {
    manifest: manifest({ required_capabilities: ['manipulation.grasp'] }),
  })
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
// §49.1's reserved channel. Three cases because the rule has three edges and each is a different
// mistake somebody could make in a second edition: refusing the token however it is asked for,
// leaving the ordinary speaker grant alone, and reporting one problem rather than two when a
// package both declares and requests it.
admissionCase('reserved-warning-channel-requested',
  'The audible warning before autonomous motion is a system function; no grant confers it, even one the operator made.',
  {
    manifest: manifest({
      embodiment: { capabilities: ['manipulation.grasp', 'hri.warning'], control_mode: 'torque' },
      requests: ['manipulation.grasp', 'hri.warning'],
    }),
    capacity: capacity({
      capabilities: ['manipulation.grasp', 'sensing.rgb', 'hri.warning'],
      granted: ['manipulation.grasp', 'hri.warning'],
    }),
  })
admissionCase('reserved-warning-channel-declared-only',
  'Declaring a need for it is refused too, or a package would install and be one manifest edit from claiming the channel.',
  {
    manifest: manifest({
      embodiment: { capabilities: ['manipulation.grasp', 'hri.warning'], control_mode: 'torque' },
    }),
    capacity: capacity({ capabilities: ['manipulation.grasp', 'sensing.rgb', 'hri.warning'] }),
  })
admissionCase('speaker-is-ordinary',
  'The rest of the speaker is an ordinary grant. A rule that refused audio outright is one nobody could ship an HRI package under.',
  {
    manifest: manifest({
      embodiment: { capabilities: ['manipulation.grasp', 'hri.speech'], control_mode: 'torque' },
      requests: ['manipulation.grasp', 'hri.speech'],
    }),
    capacity: capacity({
      capabilities: ['manipulation.grasp', 'sensing.rgb', 'hri.speech'],
      granted: ['manipulation.grasp', 'hri.speech'],
    }),
  })
admissionCase('every-finding-at-once',
  'Four problems reported together, because fixing them one deployment at a time is how people end up granting everything.',
  {
    signature: 'failed',
    observed_digest: 'sha256:other',
    manifest: manifest({ isolation: 'native', requests: ['manipulation.grasp', 'locomotion.walk'] }),
    capacity: capacity({ ram_mb: 64 }),
  })

// ── vocabulary ───────────────────────────────────────────────────────────────
//
// The closed string sets both editions must agree on, frozen as data.
//
// Every other corpus holds an edition to a *computation*: give it this input, emit these bytes. This
// one holds it to a **vocabulary**, and it exists because of a divergence that lived through a full
// green test run. `capability_reserved` was added to the Rust `PackageRejectionCode` and not to the
// reference's, and nothing could see it: the mirror pins guard the reference, and the corpus covers
// the cases it holds — none of which had ever mentioned a token invented ten minutes earlier.
//
// A missing member is now a corpus failure on the first run, with the set named. Which is the whole
// argument for putting it here rather than in a lint: §31.4 already makes the corpus the thing an
// edition must reproduce before it may carry the name, and a vocabulary is exactly the kind of
// agreement it should be carrying.
//
// The sets are emitted **in declaration order, not sorted**. Order is part of the agreement for
// several of these — `OS_TIERS` and `SCTS_AREAS` are ordered by rank, and an edition that sorted
// them alphabetically would be one whose "next tier up" arithmetic silently differed.

function vocabulary(name: string, note: string, members: readonly string[]): void {
  add('vocabulary', 'closed-sets', {
    id: name,
    note,
    input: { set: name },
    expected: canonical(members),
  })
}

vocabulary('package_rejection_codes',
  'Why an install was refused. The set this corpus was added for.',
  PACKAGE_REJECTION_CODES)
vocabulary('signature_states', 'The four states a signature can be in, plus the platform gap.',
  SIGNATURE_STATES)
vocabulary('package_kinds', 'What a package can be.', PACKAGE_KINDS)
vocabulary('isolation_modes', 'Sandboxed, or a driver.', ISOLATION_MODES)
vocabulary('reserved_capabilities', 'Tokens no grant confers (§49.1).', RESERVED_CAPABILITIES)
vocabulary('transport_verdicts', 'Ordered worst-last; the plan takes the worst binding in it.',
  TRANSPORT_VERDICTS)
vocabulary('transport_methods', 'Ordered by how much fidelity each costs.', TRANSPORT_METHODS)
vocabulary('transport_warning_codes', 'Everything a plan can report about a fit it accepted.',
  TRANSPORT_WARNING_CODES)
vocabulary('action_kinds', 'What one channel of a policy output drives.', ACTION_KINDS)
vocabulary('control_modes', 'How a joint is commanded.', CONTROL_MODES)
vocabulary('joint_types', 'The kinds of joint a descriptor can declare.', JOINT_TYPES)
vocabulary('base_kinds', 'What a body stands on.', BASE_KINDS)
vocabulary('embodiment_modalities', 'What a sensor produces.', EMBODIMENT_MODALITIES)
vocabulary('canonical_chains', 'The role vocabulary\'s chains (§5.1).', CANONICAL_CHAINS)
vocabulary('canonical_axes', 'The role vocabulary\'s axes (§5.1).', CANONICAL_AXES)
vocabulary('os_tiers', 'Ordered by rank. An edition that sorted these would differ on "next tier".',
  OS_TIERS)
vocabulary('enforcement_modes', 'Ordered by strength.', ENFORCEMENT_MODES)
vocabulary('guards', 'Every guard the kernel can run.', GUARDS)
vocabulary('fault_responses', 'What a tripped guard does.', FAULT_RESPONSES)
vocabulary('scts_suites', 'Where a conformance test runs.', SCTS_SUITES)
vocabulary('scts_areas', 'Ordered as the specification lists them.', SCTS_AREAS)

vocabulary('predicate_ops',
  'The operators a §54.2 precondition is written in. One list, or a skill refuses on one robot and '
  + 'runs on another.', PREDICATE_OPS)
vocabulary('predicate_outcomes',
  'Satisfied, unsatisfied, and the third value §54.2 requires: a precondition nobody could evaluate '
  + 'is not a precondition that passed.', OUTCOMES)
vocabulary('unevaluable_reasons',
  'Why a predicate could not be decided. §54.2 requires the refusal to name something, and a '
  + 'free-text reason is the same nothing as a free-text precondition.', UNEVALUABLE_REASONS)
vocabulary('clause_outcomes',
  'What can happen to one precondition or success criterion. The fourth is the one the predicate '
  + 'engine does not have: there was no predicate to ask about.', CLAUSE_OUTCOMES)
vocabulary('clause_origins',
  'Publisher or site. §54.2: the operator may add preconditions; the skill may not remove them.',
  CLAUSE_ORIGINS)
vocabulary('precondition_phases',
  'Before the skill starts, and every cycle while it runs.', PHASES)
vocabulary('skill_outcomes',
  'How a skill invocation ends. The Hub\'s `EpisodeOutcome` and not a second vocabulary, so an '
  + 'aborted skill becomes a labelled training example rather than a gap.', SKILL_OUTCOMES)
vocabulary('composition_faults',
  'Why a call graph is refused at install (§54.5).', COMPOSITION_FAULTS)
vocabulary('cell_faults',
  'What can be wrong with a cell descriptor (§55.2). Two of them — an unaddressable fact key and '
  + 'one that shadows a derived fact — could not have been named before §54.2 gave `env.*` a path '
  + 'vocabulary to be wrong in.', CELL_FAULTS)

// §27's machine. Eight sets, and every one of them is a place an edition could quietly disagree
// with another about what a robot is doing — which is the divergence with the worst consequence in
// this file, because a fleet console and a robot are meant to be describing the same machine.
//
// `operating_states` and `state_reasons` are emitted in **rule order** rather than sorted, and the
// order is load-bearing twice over: the states run least-capable-first, matching the order
// `evaluate` descends through its rules, and the reasons are one per rule in the same sequence. An
// edition that sorted either would still pass every evaluation case and would have lost the
// property that makes the ordering readable.
vocabulary('operating_states',
  "§27's five states plus §48.1's two, ordered least capable first — the order `evaluate` "
  + "descends through its rules.", OPERATING_STATES)
vocabulary('state_reasons',
  'Why the machine is where it is. One per rule, in rule order, because two rules produce `hold` '
  + 'and only the reason tells an operator which one — "a guard asked for a hold" and "there is '
  + 'nothing to run" are the same state and different errands.', STATE_REASONS)
vocabulary('stop_causes',
  "§27's four causes of a controlled stop, and no fifth. Closed on purpose: a fifth cause is a "
  + 'fifth thing that stops a robot.', STOP_CAUSES)
vocabulary('post_verdicts',
  "§48.2's report rolled up to the one input §27's machine takes from it. Three rather than "
  + "§48.4's four outcomes, because the four are per check.", POST_VERDICTS)
vocabulary('homing_states', 'Whether a body knows where its joints are (§48.3).', HOMING_STATES)
vocabulary('rhal_states',
  'Present, degraded, lost. The last is separate from the middle because the difference between '
  + 'them is the whole of SAFE.', RHAL_STATES)
vocabulary('policy_states',
  'Nothing to run, something that is not ready, and a lease granted with a plan resolved.',
  POLICY_STATES)
vocabulary('kernel_states',
  'Live or down. §27: "a robot with a policy and no kernel must not exist".', KERNEL_STATES)

// §54.4's lifecycle. `skill_phases` and `lifecycle_reasons` are emitted in **diagram order** and
// **rule order** respectively, and both orders are load-bearing: the phases run forwards and the
// suite asserts the machine never moves backwards through this list, so an edition that sorted them
// would be asserting a different property with the same words.
vocabulary('skill_phases',
  "§54.4's stages. `terminal` is one phase rather than three, because the outcome that distinguishes "
  + "the diagram's three arrows is `skill_outcomes` — the Hub's `EpisodeOutcome`, not a second "
  + 'vocabulary.', SKILL_PHASES)
vocabulary('lifecycle_reasons',
  'Why an invocation is where it is. Four of these end in `terminal` with `aborted`, and only the '
  + 'reason tells an operator whether to call the publisher, look at the cell, find the package '
  + 'holding the arm, or read the black box.', LIFECYCLE_REASONS)
vocabulary('lease_states',
  "§25.2's arbitration, as the one fact the lifecycle needs from it — deliberately not a copy of "
  + "the supervisor's richer outcome, because two definitions of a lease result is §31.2's failure "
  + 'in miniature.', LEASE_STATES)

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

// ── frames against the joint list, §44.3 ───────────────────────────────────────────
//
// The half `validateFrames` cannot see. `joint` and `mimics` are the two names in a dynamic frame
// that do not refer to the graph — they refer to the descriptor's joint list — and a name that
// does not resolve there leaves the frame unresolvable on every cycle. Both pose guards then trip
// on it, correctly and forever, which is a robot holding still for a reason that is a spelling.
//
// This is what `embodiment.frames.complete` measures, so the vectors are here rather than in a
// vendor's memory.

function bindingsCase(id: string, note: string, input: FrameGraph, joints: string[]): void {
  add('frames', 'bindings', {
    id, note,
    input: { graph: input, joints },
    expected: canonical(shapeFrameFault(validateFrameBindings(input, joints))),
  })
}

bindingsCase('resolves', 'Every dynamic frame names a joint the body declares.',
  ARM_FRAMES, ['right_arm.shoulder.pitch', 'right_arm.elbow.flex'])
bindingsCase('no-dynamic-frames', 'A fixed tree names no joints, so there is nothing to resolve.',
  graph([frameNode('base_link', null), frameNode('tool0', 'base_link')]), [])
bindingsCase('dangling-joint',
  'A frame moves with a joint the body does not declare, which binds to nothing and never moves.',
  ARM_FRAMES, ['right_arm.shoulder.pitch'])
bindingsCase('dangling-joint-empty-body',
  'A graph full of joints against a body that declares none — the first frame is the one named.',
  ARM_FRAMES, [])
bindingsCase('mimic-resolves', 'A linkage following a joint the body declares.',
  graph([
    frameNode('base_link', null),
    frameNode('drive', 'base_link', {
      kind: 'dynamic', joint: 'hand.finger.flex', joint_type: 'revolute', axis: [0, 1, 0],
    }),
    frameNode('follower', 'drive', {
      kind: 'dynamic', joint: 'hand.finger.distal', joint_type: 'revolute', axis: [0, 1, 0],
      mimics: 'hand.finger.flex', mimic_multiplier: -1, mimic_offset: 0,
    }),
  ]), ['hand.finger.flex', 'hand.finger.distal'])
bindingsCase('mimic-missing', 'A linkage whose driver does not exist never moves.',
  graph([
    frameNode('base_link', null),
    frameNode('follower', 'base_link', {
      kind: 'dynamic', joint: 'hand.finger.distal', joint_type: 'revolute', axis: [0, 1, 0],
      mimics: 'hand.finger.flex',
    }),
  ]), ['hand.finger.distal'])
bindingsCase('mimic-self',
  '`value = multiplier × driver + offset` over one value is a fixed point, not a linkage.',
  graph([
    frameNode('base_link', null),
    frameNode('follower', 'base_link', {
      kind: 'dynamic', joint: 'hand.finger.flex', joint_type: 'revolute', axis: [0, 1, 0],
      mimics: 'hand.finger.flex', mimic_multiplier: 2,
    }),
  ]), ['hand.finger.flex'])
bindingsCase('mimic-coefficient-without-a-mimic',
  'A coefficient with nothing to multiply describes a linkage that does not exist.',
  graph([
    frameNode('base_link', null),
    frameNode('link', 'base_link', {
      kind: 'dynamic', joint: 'hand.finger.flex', joint_type: 'revolute', axis: [0, 1, 0],
      mimic_multiplier: -1,
    }),
  ]), ['hand.finger.flex'])
bindingsCase('mimic-offset-without-a-mimic',
  'The offset alone is the same statement about a linkage that does not exist.',
  graph([
    frameNode('base_link', null),
    frameNode('link', 'base_link', { mimic_offset: 0.5 }),
  ]), [])
// The non-finite coefficient rule has **no case here, and cannot have one.** This generator writes
// every input through `JSON.stringify`, which renders infinity as `null`, so the frozen input would
// be a *different* body from the one the expectation was computed from — worse than no case at
// all, because it would fail an edition for reading the file correctly.
//
// The two editions also reach that branch differently, which is worth writing down. `1e999` is
// valid JSON syntax. `crates/skynet-json` refuses it at parse — “a literal so large it overflows to
// infinity is a corrupt corpus file, not a value” — so in the Rust edition the document is rejected
// before `validate_frame_bindings` runs at all, and the branch is reachable only from a graph built
// in memory. This oracle has no parser; whoever calls it reaches it through `JSON.parse`, which
// answers `Infinity`, and this check is the only thing standing between that and an infinite pose.
// So the rule belongs in both editions and is load-bearing in exactly one of them.
bindingsCase('a-joint-with-no-frame-is-not-a-fault',
  'A partial tree is legitimate — §44.5 has the guard report unavailable naming the frame it wanted — so “complete” means every name resolves, not that the graph covers the body.',
  ARM_FRAMES,
  ['right_arm.shoulder.pitch', 'right_arm.elbow.flex', 'hand.gripper.open'])

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
      streamsAbsent: 0,
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
    streams_absent: 0,
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
      streamsAbsent: 0,
      streams: ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'].map(name => ({ name, ageUs: 10, stale: false })),
    },
  },
  {
    kind: 'frame',
    observation: hex(BB_OBSERVATION),
    target_ns: '5230',
    worst_skew_us: 12,
    streams_omitted: 0,
    streams_absent: 0,
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
      streamsAbsent: 0,
      streams: [{ name: 'ααααααααα', ageUs: 1, stale: false }],
    },
  },
  {
    kind: 'frame',
    observation: hex(BB_OBSERVATION),
    target_ns: '6230',
    worst_skew_us: 0,
    streams_omitted: 0,
    streams_absent: 0,
    streams: [{ name: 'ααααααααα', age_us: 1, stale: false }],
  },
)

recordCase(
  'frame-with-a-dead-capture-path',
  'The record format version 1 could not write. Two of the three streams the policy declared produced '
  + 'nothing, so there is one row — and a reader of the older record would have seen a robot with one '
  + 'camera rather than a robot with two dead ones. One contributor also means there is no spread to '
  + 'report, and `worst_skew_us` is null rather than the zero that would read as perfect synchronisation.',
  BB_SHAPE, 12n, 6634n,
  {
    kind: 'frame',
    frame: {
      observation: BB_OBSERVATION,
      targetNs: 6630n,
      worstSkewUs: null,
      streamsOmitted: 0,
      streamsAbsent: 2,
      streams: [{ name: 'joints', ageUs: 900, stale: false }],
    },
  },
  {
    kind: 'frame',
    observation: hex(BB_OBSERVATION),
    target_ns: '6630',
    worst_skew_us: null,
    streams_omitted: 0,
    streams_absent: 2,
    streams: [{ name: 'joints', age_us: 900, stale: false }],
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

// ── watchdog (§46) ───────────────────────────────────────────────────────────
//
// One corpus for the chain, the command-loss verdict and the nesting, for the reason §45.7's three
// cases are one corpus: they are one question — is this robot's watchdog chain sound — and pinning
// them separately would let an edition agree about an inversion and disagree about what it costs,
// which is the half an operator reads.

function watchdogCase(
  file: string, id: string, note: string,
  onCommandLoss: string | null, windows: WatchdogWindows,
): void {
  add('watchdog', file, {
    id, note,
    input: { on_command_loss: onCommandLoss, windows },
    expected: canonical(shapeWatchdog(onCommandLoss, windows)),
  })
}

const NESTED: WatchdogWindows = {
  kernel_cycle_worst_case_ms: 2,
  envelope_watchdog_ms: 100,
  soc_wdt_ms: 250,
  drive_command_timeout_ms: 500,
}

watchdogCase('nesting', 'nested', 'The order §46.3 requires, innermost first. Every layer above is '
  + 'reachable.', 'hold', NESTED)
watchdogCase('nesting', 'wdt-pre-empts-kernel',
  'The processor resets before the kernel executes its controlled stop, so the harsher recovery '
  + "pre-empts the gentler one and the robot's best available behaviour is the one it never "
  + 'performs.',
  'hold', { ...NESTED, envelope_watchdog_ms: 250, soc_wdt_ms: 100 })
watchdogCase('nesting', 'drive-faults-first',
  'Layer 3 present, configured, and never reached: the drive faults on its own first.',
  'hold', { ...NESTED, soc_wdt_ms: 900 })
watchdogCase('nesting', 'cycle-exceeds-watchdog',
  'The kernel cannot finish a cycle inside its own command deadline, so the watchdog guard trips '
  + 'on a robot that is working correctly.',
  'hold', { ...NESTED, kernel_cycle_worst_case_ms: 120 })
watchdogCase('nesting', 'equal-windows',
  'A race whose winner is interrupt latency. A chain whose order depends on which timer fires '
  + 'first is not a chain, so the comparison is strict.',
  'hold', { ...NESTED, soc_wdt_ms: 100 })
watchdogCase('nesting', 'gap-in-the-middle',
  'An undeclared window is reported and skipped over rather than ending the walk: the pair either '
  + 'side of the gap still inverts, and that inversion is real whatever sits between them.',
  'hold', { kernel_cycle_worst_case_ms: 2, soc_wdt_ms: 1, drive_command_timeout_ms: 500 })
watchdogCase('nesting', 'nothing-declared',
  'Ordered and not complete: nothing contradicts, and nothing was established. One boolean would '
  + 'have to lie about one of them.',
  'hold', {})

watchdogCase('command-loss', 'coast',
  '§46.2, and the shape is the point: the middle rung is the unsafe one. Guest is fine, certified '
  + 'is fine because layer 4 cuts drive enable independently, and only host — where layer 3 is the '
  + 'last line — turns a reset into a fall.',
  'coast', NESTED)
watchdogCase('command-loss', 'hold', 'A body that holds takes the reset and stays where it is.',
  'hold', NESTED)
watchdogCase('command-loss', 'brake', 'Brakes engage; the recovery path is survivable.',
  'brake', NESTED)
watchdogCase('command-loss', 'fault', 'The drive faults and disables, which is a stop.',
  'fault', NESTED)
watchdogCase('command-loss', 'undeclared',
  'Silence is not `hold`. Treating it as `hold` would be inventing the most convenient answer to '
  + 'the only question layer 3 turns on.',
  null, NESTED)
watchdogCase('command-loss', 'unrecognised',
  'A value outside the vocabulary gets its own sentence: "this body says `freewheel`" and "this '
  + 'body says nothing" are different facts with different remedies.',
  'freewheel', NESTED)

// ── power and thermal (§36) ──────────────────────────────────────────────────
//
// One corpus, for the reason §46's is one: the state, the reserve, the thermal ladder and the
// inference budget are one question — what is this robot's power situation right now — and an
// edition could agree about every band and still disagree about which rung it gives up first,
// which is the half that decides whether the control loop or the telemetry uploader goes.
//
// The bands are multiples of the reserve rather than of the pack, so every case here uses one
// pack — 500 Wh with a 20 Wh reserve — and the interesting range is the bottom tenth of the gauge.
// That is not a quirk of the fixture; it is §36's opening sentence.

function powerCase(
  file: string, id: string, note: string,
  profile: PowerProfile, observation: PowerObservation,
  declaredBudgetMs?: number, observedInferenceMs?: number,
): void {
  add('power', file, {
    id, note,
    input: {
      profile, observation,
      ...(declaredBudgetMs !== undefined && { declared_budget_ms: declaredBudgetMs }),
      ...(observedInferenceMs !== undefined && { observed_inference_ms: observedInferenceMs }),
    },
    expected: canonical(shapePower(profile, observation, declaredBudgetMs, observedInferenceMs)),
  })
}

const PACK: PowerProfile = {
  time_to_safe_state_s: 2,
  safe_stop_energy_wh: 20,
  pack_capacity_wh: 500,
  thermal_throttle_c: 70,
  thermal_critical_c: 90,
  brownout_fault_codes: [4097],
}

powerCase('state', 'normal', 'Nine per cent of a 500 Wh pack is 45 Wh, which is above 2.25 × the '
  + '20 Wh reserve. Full operation, and nothing wrong.',
  PACK, { state_of_charge: 0.09 })
powerCase('state', 'low', 'Forty watt-hours: inside the low band. New task leases stop and the '
  + 'running one may finish, which is the only state where those two differ.',
  PACK, { state_of_charge: 0.08 })
powerCase('state', 'at-reserve', 'Inside the reserve band. A controlled stop, then a safe state — '
  + 'and not yet the immediate one, which is what the quarter-reserve band buys.',
  PACK, { state_of_charge: 0.045 })
powerCase('state', 'critical', 'Below the reserve. The stop is immediate and the drives are '
  + 'disabled once at rest.',
  PACK, { state_of_charge: 0.03 })
powerCase('state', 'exactly-at-reserve', 'Twenty watt-hours exactly. At reserve rather than below '
  + 'it: the other reading gives the harshest state to the body that computed its budget perfectly.',
  PACK, { energy_remaining_wh: 20 })
// The four boundary cases, and they earn their place: without them the corpus does not pin
// RESERVE_BAND or LOW_BAND at all. A tamper test that moved the reserve band from 1.25 to 1.9 left
// every other case in this file byte-identical — the fixtures all sat well inside their bands — so
// an edition could have disagreed about where `low` becomes `reserve` and reproduced the corpus
// exactly. Reported in watt-hours rather than as a fraction of the pack, so the numbers are the
// boundaries themselves rather than a multiplication that lands near them.
powerCase('state', 'just-below-the-low-band', 'A hair under 1.25 × the reserve: still at reserve, '
  + 'and the case that pins where that band ends.',
  PACK, { energy_remaining_wh: 24.999 })
powerCase('state', 'at-the-low-band', 'Exactly 1.25 × the reserve. Low, because the bands are '
  + 'inclusive at the bottom throughout — the same rule that puts a body exactly at its reserve at '
  + 'reserve rather than below it.',
  PACK, { energy_remaining_wh: 25 })
powerCase('state', 'just-below-normal', 'A hair under 2.25 × the reserve. Still low, and the case '
  + 'that pins where the low band ends.',
  PACK, { energy_remaining_wh: 44.999 })
powerCase('state', 'at-normal', 'Exactly 2.25 × the reserve: one whole further safe stop in hand '
  + 'above the reserve band, which is the width §36.1\'s `low` row needs to keep its promise.',
  PACK, { energy_remaining_wh: 45 })

powerCase('state', 'energy-beats-fraction', 'Both reported and they disagree. The gauge reporting '
  + 'watt-hours wins, because whatever told it the capacity is closer to the pack than a '
  + 'descriptor field is.',
  PACK, { state_of_charge: 0.9, energy_remaining_wh: 10 })

powerCase('brownout', 'declared-code', '§36.4: the bus says sag and the gauge says full. The gauge '
  + 'is not consulted, because a gauge that disagrees with the bus is a gauge that is wrong.',
  PACK, { state_of_charge: 1, rhal_degraded: true, fault_code: 4097 })
powerCase('brownout', 'undeclared-code', 'A degraded RHAL with a code this vendor did not call a '
  + 'brownout. The OS matches a declared list; §24 forbids it interpreting a vendor code, so a '
  + 'code that is not on the list is not a sag.',
  PACK, { state_of_charge: 1, rhal_degraded: true, fault_code: 9999 })
powerCase('brownout', 'no-codes-declared', 'The gap §36.4 has until a vendor fills it in: the RHAL '
  + 'is degraded and nothing on this body can tell a voltage sag from a dead encoder. Reported, '
  + 'and the robot keeps working — a hole in a declaration is not a reason to stop a machine.',
  { ...PACK, brownout_fault_codes: [] },
  { state_of_charge: 0.9, rhal_degraded: true, fault_code: 4097 })
powerCase('brownout', 'no-gauge-at-all', 'The override does not consult the gauge, and does not '
  + 'consult the absence of one either.',
  PACK, { rhal_degraded: true, fault_code: 4097 })

powerCase('unmetered', 'no-pack', 'An arm bolted to a bench and run off mains. §36.1 has four '
  + 'states and all four assume a battery; this is the fifth, and it grants leases, because a body '
  + 'with no pack is not a body in trouble.',
  { thermal_throttle_c: 70, thermal_critical_c: 90 }, {})
powerCase('unmetered', 'gauge-silent', 'A body that declares a pack and got nothing back. Not '
  + 'critical — missing evidence is never a positive finding — and not normal either: no new lease '
  + 'until something answers for the pack.',
  PACK, {})
powerCase('unmetered', 'capacity-undeclared', 'A fraction with nothing to scale it by. The remedy '
  + 'is a descriptor field, and the fault says which one.',
  { ...PACK, pack_capacity_wh: undefined }, { state_of_charge: 0.5 })
powerCase('unmetered', 'reserve-undeclared', 'A pack with no safe-stop energy: §36.2 has nothing '
  + 'to compute a reserve from, so the four states have no boundary. This is the field '
  + '`power.safe_state.reachable` has been measuring against since Part II, with nothing to read.',
  { ...PACK, safe_stop_energy_wh: undefined }, { state_of_charge: 0.5 })
powerCase('unmetered', 'charge-out-of-range', 'A state of charge of 1.4. Discarded rather than '
  + 'clamped — a clamped 1.4 is a full battery — and reported as a gauge to fix rather than as a '
  + 'descriptor to fill in, which is the distinction `reading_unusable` exists for.',
  PACK, { state_of_charge: 1.4 })

powerCase('thermal', 'nominal', 'Warm and under the threshold. Nothing is given up.',
  PACK, { state_of_charge: 0.9, temperature_c: 60 })
powerCase('thermal', 'brief-throttle', 'Above the throttle point for four seconds. The three '
  + 'housekeeping rungs go and DEGRADED does not: a thermal transient from one heavy motion is not '
  + 'a degraded robot.',
  PACK, { state_of_charge: 0.9, temperature_c: 75, throttled_for_ms: 4_000 })
powerCase('thermal', 'sustained-throttle', 'Thirty seconds. §36.3 raises DEGRADED, which is the '
  + 'impaired *running* state — leases are still granted, because a robot that stops for being '
  + 'warm is a robot nobody deploys.',
  PACK, { state_of_charge: 0.9, temperature_c: 75, throttled_for_ms: 30_000 })
powerCase('thermal', 'critical', 'Every rung of the ladder, inference included. Never the RHAL, '
  + 'the kernel or the control loop: there is no thermal situation in which giving up the safety '
  + 'envelope is the right trade.',
  PACK, { state_of_charge: 0.9, temperature_c: 95, throttled_for_ms: 120_000 })
powerCase('thermal', 'undeclared-thresholds', 'Something is reading 95 °C and nothing on this body '
  + 'says what hot is. Nothing is shed and the gap is named: 85 °C is right for a CPU package and '
  + 'fifty degrees wrong for a harmonic drive.',
  { ...PACK, thermal_throttle_c: undefined, thermal_critical_c: undefined },
  { state_of_charge: 0.9, temperature_c: 95 })

powerCase('budget', 'inside', 'A loop comfortably inside its declared budget, on a warm robot. '
  + 'Thermal state does not block a package; a missed deadline does.',
  PACK, { state_of_charge: 0.9, temperature_c: 75, throttled_for_ms: 60_000 }, 20, 18)
powerCase('budget', 'missed', 'The loop is late. §36.3 throttles inference before control, so this '
  + 'is a package refused rather than a robot in danger — and the sentence names both numbers.',
  PACK, { state_of_charge: 0.9, temperature_c: 75, throttled_for_ms: 60_000 }, 20, 31)
powerCase('budget', 'undeclared', 'A package that declares no budget cannot miss one. Blocking on '
  + 'an absence is the mistake this whole document is written against.',
  PACK, { state_of_charge: 0.9 }, undefined, 400)
powerCase('budget', 'unmeasured', 'A budget with nothing measured against it. Same rule, other '
  + 'direction.',
  PACK, { state_of_charge: 0.9 }, 20, undefined)

// ── calibration (§45.7) ──────────────────────────────────────────────────────
//
// §45.7 asks for three conformance additions and they are one question asked of one input, so they
// are one corpus: given these records and this moment, what is believed and which guards lose their
// input. Splitting them would let an edition pass `expiry_enforced` while disagreeing about which
// guard the expired set feeds — which is the half that matters to whoever is standing next to the
// robot.

const CALIBRATION_TAKEN: Conditions = {
  at_s: 1_700_000_000,
  temperature_c: 22,
  joint_travel_rad: 1_000,
  tool: 'gripper-2f85',
  payload_kg: 0,
}

function calibrationRecord(
  set: string, overrides: Partial<CalibrationRecord> = {},
): CalibrationRecord {
  return {
    set,
    method: 'charuco-12x9, 40 poses',
    residual: 0.4,
    residual_threshold: 1,
    conditions: CALIBRATION_TAKEN,
    operator: 'alice',
    policy: {},
    events_since: [],
    ...overrides,
  }
}

function calibrationCase(
  file: string, id: string, note: string,
  records: CalibrationRecord[], now: Conditions, applicable?: string[],
): void {
  const judged: Array<[string, Validity]> = records.map(
    record => [record.set, evaluateCalibration(record, now)])
  const sets = applicable ?? allSets()
  add('calibration', file, {
    id, note,
    input: { now, records, ...(applicable ? { applicable } : {}) },
    expected: canonical(shapeCalibration(judged, guardsUnavailable(judged, sets))),
  })
}

const CALIBRATION_LATER: Conditions = {
  at_s: 1_700_000_000 + 3_600,
  temperature_c: 22,
  joint_travel_rad: 1_000,
}

// `calibration.provenance` — every stored record carries a method and a residual, or it is refused.
calibrationCase('provenance', 'complete',
  'Method, residual and threshold all present: judged on its policy like any other record.',
  [calibrationRecord('camera_extrinsics', { policy: { max_age_s: 86_400 } })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('provenance', 'no-method',
  'A measurement nobody can repeat. Unusable rather than believed, and its guards go with it.',
  [calibrationRecord('camera_extrinsics', { method: null })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('provenance', 'no-residual',
  'A measurement nobody can grade — the other half of §45.3, and a different sentence.',
  [calibrationRecord('camera_extrinsics', { residual: null })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('provenance', 'residual-over-threshold',
  'Twelve millimetres of reprojection error against a threshold of two: failed, not expired. It '
  + 'was never a calibration, so no amount of freshness makes it one.',
  [calibrationRecord('camera_extrinsics', { residual: 12, residual_threshold: 2 })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('provenance', 'absent',
  'Nobody ever measured this set. Absence is not validity, and the guards say so by name.',
  [], CALIBRATION_LATER, ['imu_bias'])

// `calibration.expiry_enforced` — a deliberately expired record disables its dependent guards, and
// the guards report the parameter set by name.
calibrationCase('expiry', 'within-age',
  'One hour into a one-day validity. Valid, and the checked list says what was actually tested.',
  [calibrationRecord('camera_extrinsics', { policy: { max_age_s: 86_400 } })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('expiry', 'past-age',
  'Past its elapsed-time backstop: proximity and workspace both lose their input, by name.',
  [calibrationRecord('camera_extrinsics', { policy: { max_age_s: 600 } })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('expiry', 'no-policy',
  'A record that expires on nothing is valid and says nothing was checked — which is how a console '
  + 'tells "still valid" from "nobody set a policy".',
  [calibrationRecord('joint_zeros')], CALIBRATION_LATER, ['joint_zeros'])
calibrationCase('expiry', 'joint-travel',
  'Joint zeros drift with travel, not with the calendar (§45.2).',
  [calibrationRecord('joint_zeros', { policy: { max_joint_travel_rad: 500 } })],
  { ...CALIBRATION_LATER, joint_travel_rad: 2_000 }, ['joint_zeros'])
calibrationCase('expiry', 'temperature-delta',
  'F/T bias drifts with temperature continuously, so the delta is the rule and the direction does '
  + 'not matter.',
  [calibrationRecord('force_torque_bias', { policy: { max_temperature_delta_c: 5 } })],
  { ...CALIBRATION_LATER, temperature_c: 40 }, ['force_torque_bias'])
calibrationCase('expiry', 'temperature-unreadable',
  'The rule that keeps an expiry policy from switching itself off: a robot whose temperature sensor '
  + 'has failed is precisely the robot whose bias should not be trusted.',
  [calibrationRecord('force_torque_bias', { policy: { max_temperature_delta_c: 5 } })],
  { at_s: CALIBRATION_LATER.at_s }, ['force_torque_bias'])
calibrationCase('expiry', 'two-sets-one-guard',
  'Workspace starved by expired joint zeros and an expired tool offset is two measurements '
  + 'somebody has to take, so it is two lines.',
  [
    calibrationRecord('joint_zeros', { policy: { max_age_s: 600 } }),
    calibrationRecord('tool_offset', { policy: { max_age_s: 600 } }),
  ],
  CALIBRATION_LATER, ['joint_zeros', 'tool_offset'])

// `calibration.event_invalidation` — a simulated collision invalidates the affected sets.
calibrationCase('events', 'collision',
  'Measured a minute ago and one impact old. §45.4: a robot that has just hit something has, by '
  + 'definition, possibly moved its cameras.',
  [calibrationRecord('camera_extrinsics', {
    policy: { max_age_s: 86_400, invalidating_events: ['collision'] },
    events_since: ['collision'],
  })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('events', 'unlisted-event',
  'A tool change does not move a camera. Expiring everything on every event would train an '
  + 'operator to recalibrate nothing.',
  [calibrationRecord('camera_extrinsics', {
    policy: { max_age_s: 86_400, invalidating_events: ['collision'] },
    events_since: ['tool_change'],
  })],
  CALIBRATION_LATER, ['camera_extrinsics'])
calibrationCase('events', 'tool-change-on-tool-offset',
  'The same event against the set it does invalidate.',
  [calibrationRecord('tool_offset', {
    policy: { max_age_s: 86_400, invalidating_events: ['tool_change'] },
    events_since: ['tool_change'],
  })],
  CALIBRATION_LATER, ['tool_offset'])
calibrationCase('events', 'collision-beats-freshness',
  'The event is checked before age, because it is a step rather than a slope: this record is sixty '
  + 'seconds old and worthless.',
  [calibrationRecord('camera_extrinsics', {
    conditions: { ...CALIBRATION_TAKEN, at_s: CALIBRATION_LATER.at_s! - 60 },
    policy: { max_age_s: 86_400, invalidating_events: ['collision', 'estop_under_load'] },
    events_since: ['estop_under_load'],
  })],
  CALIBRATION_LATER, ['camera_extrinsics'])

// The body decides which sets are applicable at all. A fixed-base arm reporting `balance`
// unavailable for want of an IMU bias it can never hold is a page of guards nobody can restore.
calibrationCase('events', 'arm-without-an-imu',
  'A manipulator with a camera: seven sets exist, four apply, and `balance` is not among them.',
  [], CALIBRATION_LATER, setsFor(['manipulation.grasp', 'sensing.rgb'], 'fixed'))
calibrationCase('events', 'a-body-that-declares-nothing',
  'Every set applies, because over-reporting what is missing is the conservative direction.',
  [], CALIBRATION_LATER, setsFor([], null))

// ── observation (§25.1) ────────────────────────────────────────────
//
// The corpus §25.1 has been owed since Part II. A skewed frame is the failure that does not
// announce itself — it presents as a policy that grasps slightly late, and sends somebody to
// retrain a model that was fine — so the numbers the assembler reports are the only evidence it
// was ever wrong, and two editions disagreeing about them is two robots reporting two different
// histories of the same incident.
//
// Two files, because there are two questions and the second is not a detail of the first: given a
// target instant, what goes in the frame; and given a set of streams, which instant should a live
// loop even aim at. An edition could agree about every assembled frame and pick a different target
// every cycle, which would be a disagreement about every frame in the field and none in the corpus.

const MS = 1_000_000
/** One second in, so a latency correction can move a sample back without going negative. */
const T = 1_000 * MS

function stream(
  name: string, modality: string, latency_ms: number | null,
  budget_ms: number | null, stamps: number[], uncertainty_ms?: number,
): ObservationStream {
  return {
    name,
    modality,
    latency_ms,
    budget_ms,
    ...(uncertainty_ms === undefined ? {} : { uncertainty_ms }),
    // Sequence numbers ascend with the stamps, which is what a capture ring produces and what makes
    // a chosen `seq` legible in the expected bytes.
    samples: stamps.map((stamp, index) => ({ stamp_ns: stamp, seq: index + 1 })),
  }
}

function observationCase(
  file: string, id: string, note: string, targetNs: number, streams: ObservationStream[],
): void {
  add('observation', file, {
    id, note,
    input: { target_ns: targetNs, streams },
    expected: canonical(shapeObservation(assembleObservation(targetNs, streams))),
  })
}

function targetCase(
  id: string, note: string, nowNs: number, streams: ObservationStream[],
): void {
  add('observation', 'target', {
    id, note,
    input: { now_ns: nowNs, streams },
    expected: canonical({ target_ns: latestCommonInstant(nowNs, streams) }),
  })
}

// The frame §25.1 describes, assembled correctly: a camera 28 ms behind the world, joint state at
// 1 kHz 1 ms behind it, and an IMU. Every stream inside its budget.
observationCase('frames', 'synchronised',
  'Three streams at three rates, and the correction inverts the order. By the stamps alone the '
  + 'camera looks the freshest thing in the frame — 15 ms ahead of the joint sample — and once each '
  + 'stream is moved back by its own declared latency the camera turns out to describe the '
  + 'earliest instant in it. That inversion is what §25.1 is for: taking the latest value from each '
  + 'stream would have built the frame around the stalest one while believing the opposite.',
  T, [
    stream('head_cam', 'rgb', 28, 80, [T - 18 * MS, T + 15 * MS]),
    stream('joints', 'joint_state', 1, 10, [T - 500_000, T + 500_000]),
    stream('imu', 'imu', 2, 20, [T - 6 * MS, T - 1 * MS]),
  ])

observationCase('frames', 'the-frame-that-never-existed',
  '§25.1’s opening failure, measured: the camera’s freshest sample is two frames old and the '
  + 'joints are current, so the arm is where it is now and the image shows where it was. Nothing is '
  + 'broken, every budget holds, and the skew is the whole of what the assembler exists to say.',
  T, [
    stream('head_cam', 'rgb', 28, 120, [T - 38 * MS]),
    stream('joints', 'joint_state', 1, 10, [T + 1 * MS]),
  ])

observationCase('frames', 'stale-beyond-budget',
  'The camera is 138 ms behind the instant and the policy declared 80. §25.1: exceeding it is a '
  + 'hold, not a silently older frame.',
  T, [
    stream('head_cam', 'rgb', 28, 80, [T - 110 * MS]),
    stream('joints', 'joint_state', 1, 10, [T + 1 * MS]),
  ])

observationCase('frames', 'no-sample',
  'A stream the policy declared and the pipeline is not producing. A hold with its own code, '
  + 'because an empty capture path and a slow one send a person to two different places.',
  T, [
    stream('head_cam', 'rgb', 28, 80, []),
    stream('joints', 'joint_state', 1, 10, [T + 1 * MS]),
  ])

observationCase('frames', 'undeclared-latency',
  'The camera declares no latency, so it is assembled at its raw stamp and named. The measured skew '
  + 'is still reported and the bound on it is null: the camera’s true instant could be anywhere, '
  + 'and a number claiming otherwise is worse than no number.',
  T, [
    stream('head_cam', 'rgb', null, 200, [T - 10 * MS]),
    stream('joints', 'joint_state', 1, 10, [T + 1 * MS]),
  ])

observationCase('frames', 'estimated-latency',
  '§47.2: a backend that cannot supply a capture timestamp records an estimate, and the frame '
  + 'carries a larger uncertainty which flows into the skew. The measurement does not move; the '
  + 'bound does.',
  T, [
    stream('head_cam', 'rgb', 28, 200, [T - 10 * MS], 6),
    stream('joints', 'joint_state', 1, 10, [T + 1 * MS], 0.25),
  ])

observationCase('frames', 'undeclared-budget',
  'A policy that declared no staleness requirement for a stream. Not a hold — nothing here can say '
  + 'whether the sample is good enough — and named separately from an undeclared latency, because '
  + 'the gap is in the package’s declaration rather than the body’s and the remedy is a different '
  + 'person.',
  T, [
    stream('head_cam', 'rgb', 28, null, [T - 200 * MS]),
    stream('joints', 'joint_state', 1, 10, [T + 1 * MS]),
  ])

observationCase('frames', 'tie-takes-the-earlier',
  'Two samples 40 ms either side of the target are equally good by the distance rule and are not '
  + 'equally good in any other sense: the earlier one describes something that had already happened '
  + 'at the instant the frame claims to be.',
  T, [stream('joints', 'joint_state', 0, null, [T - 40 * MS, T + 40 * MS])])

observationCase('frames', 'a-sample-from-the-future',
  'Replay, or a clock that ran backwards. The nearer sample is ahead of the target, the age is '
  + 'negative, and it is deliberately not reported as staleness — the budget is one-sided, and '
  + 'calling this stale would send somebody to look at a capture pipeline that is working.',
  T, [stream('joints', 'joint_state', 0, 10, [T - 90 * MS, T + 4 * MS])])

observationCase('frames', 'one-contributing-stream',
  'The spread of a single instant is not zero, it is undefined. Zero would let a frame with one '
  + 'working stream look perfectly synchronised, which is the presentation §25.1 is written '
  + 'against.',
  T, [
    stream('head_cam', 'rgb', 28, 200, [T - 10 * MS]),
    stream('depth', 'depth', 40, 200, []),
  ])

observationCase('frames', 'no-streams-at-all',
  'A policy that requires no observation. Deliverable, and every aggregate that would have to '
  + 'invent a value is null or empty rather than zero.',
  T, [])

observationCase('frames', 'sub-millisecond-latency',
  'A 0.35 ms correction, to pin the nanosecond rounding: the conversion is one multiplication and a '
  + 'round-half-away-from-zero written out in both editions rather than taken from a platform '
  + 'library.',
  T, [
    stream('joints', 'joint_state', 0.35, 5, [T + 350_000]),
    stream('imu', 'imu', 0.15, 5, [T + 150_000]),
  ])

targetCase('every-stream-can-answer',
  'The slowest stream decides. The camera describes 0.972 s and the joints describe 1.019 s, so the '
  + 'most recent frame everybody can answer for is at 0.972 s — and aiming at now instead would '
  + 'make the camera the stalest stream in every frame for ever, a problem the loop created.',
  T + 100 * MS, [
    stream('head_cam', 'rgb', 28, 80, [T]),
    stream('joints', 'joint_state', 1, 10, [T + 20 * MS]),
  ])

targetCase('a-stream-with-nothing-yet',
  'Null rather than a later instant, and null rather than an instant that quietly drops the stream. '
  + 'It is the same fact the assembler reports as a `no_sample` hold, and it is worth having before '
  + 'building a frame nobody can use.',
  T, [
    stream('head_cam', 'rgb', 0, 80, [T - 10 * MS]),
    stream('depth', 'depth', 0, 80, [T + 500 * MS]),
  ])

targetCase('no-streams',
  'A policy requiring no observation: every instant is common to all of no streams, so the answer '
  + 'is the one asked about.',
  T, [])

targetCase('latency-larger-than-the-clock',
  'A declared latency past the start of the monotonic clock. The correction is still right and the '
  + 'instant it names is one no `u64` can express, so the caller gets null rather than a wrapped '
  + 'value.',
  T, [stream('head_cam', 'rgb', 2_000, 80, [T])])

// The fuzz half, generated here rather than beside the other two because it needs this section's
// `stream` helper and its target instant, and a `const` cannot be read from further up the file.
//
// The distribution is drawn deliberately, for the reason the cycle fuzz gives: a uniform draw over
// plausible-looking numbers produced a corpus in which almost every frame was deliverable, so the
// two hold paths and the two unbounded ones were pinned by the curated cases and by nothing else.
// The coverage is counted and printed at generation time, so a change that quietly flattens it is
// visible when it happens rather than three sessions later.

const obsRand = lcg(0x0b5efeed)
const MODALITIES: Array<[string, string, number]> = [
  // name, modality, a plausible declared latency in ms
  ['head_cam', 'rgb', 28],
  ['wrist_cam', 'rgb', 22],
  ['depth', 'depth', 40],
  ['joints', 'joint_state', 1],
  ['imu', 'imu', 2],
  ['wrist_ft', 'force_torque', 3],
]

const obsCoverage: Record<string, number> = {
  ok: 0, hold: 0, stale: 0, no_sample: 0,
  undeclared_latency: 0, undeclared_budget: 0, negative_age: 0, tie: 0,
}

for (let n = 0; n < 48; n += 1) {
  const count = 1 + Math.floor(obsRand() * 4)
  const streams: ObservationStream[] = []
  const used = new Set<string>()
  for (let i = 0; i < count; i += 1) {
    const [name, modality, typical] = MODALITIES[Math.floor(obsRand() * MODALITIES.length)]
    if (used.has(name)) continue
    used.add(name)

    // A fifth of streams declare no latency, and a fifth of the rest carry an uncertainty. Both are
    // drawn often enough that the unbounded-skew path is reached by generated input rather than
    // only by the case written to reach it.
    const latency = obsRand() < 0.2 ? null : typical
    const uncertainty = latency !== null && obsRand() < 0.25
      ? Math.round(obsRand() * 60) / 10
      : undefined
    // A sixth declare no budget, and the rest draw one that a stale sample can plausibly exceed.
    const budget = obsRand() < 0.17 ? null : 10 + Math.round(obsRand() * 20) * 10

    const stamps: number[] = []
    const roll = obsRand()
    if (roll < 0.12) {
      // An empty stream: the capture path is not running.
    } else if (roll < 0.24) {
      // A symmetric pair, so the tie-break is exercised by generated input. Without this the
      // distance between two samples is almost never exactly equal and the rule is pinned by one
      // curated case.
      const offset = (1 + Math.floor(obsRand() * 40)) * MS
      const centre = latency === null ? T : T + latency * MS
      stamps.push(centre - offset, centre + offset)
    } else {
      // The band is drawn **per stream**, not per sample, and that is the whole of what makes
      // staleness reachable. Drawn per sample, a stream with three samples almost always holds one
      // fresh enough to be the nearest, so the first version of this generator produced three stale
      // frames in forty-eight and reported coverage of a path it was barely touching.
      const lagging = obsRand() < 0.3
      const samples = 1 + Math.floor(obsRand() * 3)
      for (let k = 0; k < samples; k += 1) {
        const lagMs = lagging
          ? 60 + Math.round(obsRand() * 260)
          : Math.round(obsRand() * 40) - 4
        stamps.push(T - Math.round(lagMs * MS) + (latency === null ? 0 : latency * MS))
      }
    }
    streams.push(stream(name, modality, latency, budget, stamps, uncertainty))
  }

  const frame = assembleObservation(T, streams)
  obsCoverage[frame.verdict] += 1
  for (const hold of frame.holds) obsCoverage[hold.reason] += 1
  if (frame.undeclared_latency.length > 0) obsCoverage.undeclared_latency += 1
  if (frame.undeclared_budget.length > 0) obsCoverage.undeclared_budget += 1
  if (frame.streams.some(one => one.chosen !== null && one.chosen.age_ms < 0)) {
    obsCoverage.negative_age += 1
  }
  if (streams.some(one => one.samples.length === 2)) obsCoverage.tie += 1

  add('fuzz', 'observation', {
    id: `seeded-${n}`,
    note: 'Generated from seed 0x0b5efeed.',
    input: { target_ns: T, streams },
    expected: canonical(shapeObservation(frame)),
  })
}

// ── predicate ────────────────────────────────────────────────────────────────
//
// §54.2's operator set, over a **live** context rather than a manifest one, because that is the
// half the OS added: the Hub evaluates these against declared facts, and the skill runtime
// evaluates the same shapes against a robot that is standing there.
//
// Every case pins the boolean **and** the three-valued outcome. `evaluatePredicate` is defined as
// `satisfied(…)` of the outcome, so pinning them together is what makes "the refinement cannot
// disagree with the boolean" a property an edition is held to rather than a claim in a comment.

/**
 * One cell, one robot, one skill — the context §54.2 describes.
 *
 * Deliberately full of the awkward values: a percentage reported as a string, a calibration set
 * that has expired, an explicit `null`, an empty list, a blank string. Those are where two editions
 * disagree, not on `battery_percent: 62`.
 */
const CONTEXT = {
  robot: {
    tier: 'host',
    runtime: '1.4.2',
    capabilities: ['manipulation.grasp', 'sensing.camera.depth'],
    state: {
      mode: 'idle',
      gripper: 'open',
      battery_percent: 62,
      reported_percent: '17',
      empty: [] as string[],
      blank: '   ',
      zero: 0,
      nothing: null,
    },
    guards: { active: ['workspace', 'velocity', 'collision'] },
    calibration: {
      kinematic: { valid: true, age_days: 12 },
      torque: { valid: false, age_days: 400 },
    },
  },
  skill: {
    grants: ['actuation.arm'],
    needs: ['manipulation.grasp'],
    also: ['manipulation.grasp'],
    range: '^1.0.0',
  },
  env: {
    door_closed: true,
    fixture: 'jig-a',
    temperature_c: '21.5',
    hex: '0x1f',
    exponent: '1e3',
    infinite: 'Infinity',
  },
}

function predicateCase(id: string, note: string, predicate: unknown, context: unknown = CONTEXT): void {
  const outcome = evaluatePredicateOutcome(predicate as never, context)
  add('predicate', 'evaluate', {
    id,
    note,
    input: { predicate, context },
    expected: canonical(shapePredicate(outcome, evaluatePredicate(predicate as never, context))),
  })
}

// The operators, each holding and each not.

predicateCase('exists-value', 'A path with a value exists.',
  { op: 'exists', path: 'robot.state.gripper' })
predicateCase('exists-zero', 'Zero is a value. A robot at 0% has reported its battery.',
  { op: 'exists', path: 'robot.state.zero' })
predicateCase('exists-null', 'An explicit null does not exist — somebody looked and found nothing.',
  { op: 'exists', path: 'robot.state.nothing' })
predicateCase('exists-empty-list', 'An empty list does not exist. No guards active is not "guards".',
  { op: 'exists', path: 'robot.state.empty' })
predicateCase('exists-blank-string', 'A string of spaces does not exist.',
  { op: 'exists', path: 'robot.state.blank' })
predicateCase('exists-missing', 'The escape hatch: `exists` is the one operator an absent path does '
  + 'not refuse. It answers unsatisfied, so `not(exists(p))` still means what its author meant.',
  { op: 'exists', path: 'robot.state.absent' })

predicateCase('eq-string', 'The ordinary case.',
  { op: 'eq', path: 'robot.state.mode', value: 'idle' })
predicateCase('eq-mismatch', 'A different value.',
  { op: 'eq', path: 'robot.state.mode', value: 'estop' })
predicateCase('eq-null-literal', 'A literal null equals an explicit null: both are primitives.',
  { op: 'eq', path: 'robot.state.nothing', value: null })
predicateCase('eq-literal-array-never-matches',
  'A literal array in a rule is never equal to anything. `===` on two objects is reference '
  + 'equality, and a rule\'s literal was never the context\'s object. `includes` is the operator '
  + 'for this intent, and finding that out here is cheaper than finding it out in a cell.',
  { op: 'eq', path: 'skill.needs', value: ['manipulation.grasp'] })
predicateCase('eq-same-path-reference',
  'Two operands read from the same path are one object, so they are equal even though neither is a '
  + 'primitive. This is the only way an object comparison can hold.',
  { op: 'eq', path: 'skill.needs', value: { __ref: 'skill.needs' } })
predicateCase('eq-structurally-equal-different-paths',
  'Two arrays with identical contents at different paths are not equal, because `===` is not a '
  + 'deep comparison. `skill.also` holds exactly what `skill.needs` holds.',
  { op: 'eq', path: 'skill.needs', value: { __ref: 'skill.also' } })
predicateCase('eq-element-of-a-collection',
  'A path may address an array member, and the member is the same object the collection holds.',
  { op: 'eq', path: 'robot.guards.active.0', value: 'workspace' })

predicateCase('neq-holds', 'Not in estop.',
  { op: 'neq', path: 'robot.state.mode', value: 'estop' })
predicateCase('neq-does-not-hold', 'It is idle, so it is not not-idle.',
  { op: 'neq', path: 'robot.state.mode', value: 'idle' })
predicateCase('neq-unresolved-refuses',
  'The headline change. `neq` against a path that names nothing used to answer **true** — '
  + '`undefined !== "estop"` — so a rule reading "the robot is not in estop" passed on a robot that '
  + 'had never reported a mode. It now refuses and names the path.',
  { op: 'neq', path: 'robot.state.absent', value: 'estop' })

predicateCase('in-holds', 'The value is one of the listed.',
  { op: 'in', path: 'robot.state.gripper', value: ['open', 'closed'] })
predicateCase('in-does-not-hold', 'The value is not.',
  { op: 'in', path: 'robot.state.mode', value: ['estop', 'fault'] })
predicateCase('in-scalar-value-is-a-one-element-list', 'A non-array right-hand side is wrapped.',
  { op: 'in', path: 'robot.state.mode', value: 'idle' })

predicateCase('includes-holds', 'The collection contains the rule\'s value.',
  { op: 'includes', path: 'robot.guards.active', value: 'workspace' })
predicateCase('includes-does-not-hold', 'It does not.',
  { op: 'includes', path: 'robot.guards.active', value: 'pose' })
predicateCase('includes-all-holds', 'Every capability the skill needs is present.',
  { op: 'includes_all', path: 'robot.capabilities', value: { __ref: 'skill.needs' } })
predicateCase('includes-all-does-not-hold', 'One is missing.',
  { op: 'includes_all', path: 'robot.capabilities', value: ['manipulation.grasp', 'locomotion.walk'] })
predicateCase('includes-all-empty-requirement',
  'An empty requirement is vacuously satisfied: nothing was asked for. This is now reachable only '
  + 'from a requirement that *is* empty, never from one nobody could read.',
  { op: 'includes_all', path: 'robot.capabilities', value: [] })
predicateCase('includes-all-unresolved-requirement-refuses',
  'The rule this module\'s own documentation uses as its example — "robot capabilities must cover '
  + 'whatever this policy declares" — used to **pass** when the policy declared nothing readable, '
  + 'because `every` over an empty list is true. It now refuses and names the reference.',
  { op: 'includes_all', path: 'robot.capabilities', value: { __ref: 'skill.absent_list' } })
predicateCase('includes-any-holds', 'At least one.',
  { op: 'includes_any', path: 'robot.guards.active', value: ['pose', 'velocity'] })
predicateCase('includes-any-does-not-hold', 'None of them.',
  { op: 'includes_any', path: 'robot.guards.active', value: ['pose', 'payload'] })
predicateCase('includes-any-empty-requirement', 'Nothing was asked for.',
  { op: 'includes_any', path: 'robot.guards.active', value: [] })

predicateCase('gte-number', 'Battery above reserve.',
  { op: 'gte', path: 'robot.state.battery_percent', value: 20 })
predicateCase('lt-number', 'And the same fact from the other side.',
  { op: 'lt', path: 'robot.state.battery_percent', value: 20 })
predicateCase('gt-boundary', 'Strictly greater is not greater-or-equal.',
  { op: 'gt', path: 'robot.state.battery_percent', value: 62 })
predicateCase('lte-boundary', 'And the boundary itself.',
  { op: 'lte', path: 'robot.state.battery_percent', value: 62 })
predicateCase('gte-numeric-string',
  'A percentage reported as a string still compares. Robots report numbers as strings, and an '
  + 'edition that refused this one and accepted the next would be a robot that stops for a quote.',
  { op: 'gte', path: 'robot.state.reported_percent', value: 10 })
predicateCase('gte-decimal-string', 'A decimal string.',
  { op: 'gte', path: 'env.temperature_c', value: 21 })
predicateCase('gte-hex-string', '`Number("0x1f")` is 31, and this is the reference being mirrored '
  + 'rather than a shape anybody should write.',
  { op: 'gte', path: 'env.hex', value: 30 })
predicateCase('gte-exponent-string', '`Number("1e3")` is 1000.',
  { op: 'gte', path: 'env.exponent', value: 999 })
predicateCase('gte-infinity-string-refuses',
  '`Number("Infinity")` is not finite, and a ceiling nothing can exceed is not a comparison.',
  { op: 'gte', path: 'env.infinite', value: 5 })
predicateCase('gte-non-numeric-string-refuses', 'A fixture name is not a number.',
  { op: 'gte', path: 'env.fixture', value: 1 })
predicateCase('gte-blank-string-refuses', 'A blank string is not zero here.',
  { op: 'gte', path: 'robot.state.blank', value: 0 })
predicateCase('gte-boolean-refuses', 'A boolean is not coerced. `Number(true)` is 1 in JavaScript '
  + 'and the reference never reaches it: only numbers and strings are read.',
  { op: 'gte', path: 'env.door_closed', value: 0 })
predicateCase('gte-null-refuses', 'An explicit null is present and is not a number.',
  { op: 'gte', path: 'robot.state.nothing', value: 0 })
predicateCase('gte-unresolved-right-refuses', 'The reference names the side that failed, and here '
  + 'it is the rule\'s own reference rather than the robot\'s value.',
  { op: 'gte', path: 'robot.state.battery_percent', value: { __ref: 'skill.reserve_percent' } })

predicateCase('semver-holds', 'The runtime is inside the skill\'s window.',
  { op: 'semver_satisfies', path: 'robot.runtime', value: { __ref: 'skill.range' } })
predicateCase('semver-does-not-hold', 'And outside a different one.',
  { op: 'semver_satisfies', path: 'robot.runtime', value: '>=2.0.0' })
predicateCase('semver-wildcard', 'A wildcard constrains nothing and is a valid range.',
  { op: 'semver_satisfies', path: 'robot.runtime', value: '*' })
predicateCase('semver-unreadable-version-refuses',
  'A version this edition cannot parse is a fact about the declaration, not a version that fell '
  + 'outside the window — so it is named rather than counted as a miss.',
  { op: 'semver_satisfies', path: 'robot.state.mode', value: '^1.0.0' })
predicateCase('semver-unreadable-range-refuses', 'And the same for the range.',
  { op: 'semver_satisfies', path: 'robot.runtime', value: 'garbage' })
predicateCase('semver-range-with-one-unreadable-alternative-refuses',
  'Stricter than `satisfies`, deliberately: `>=1.0.0 || garbage` is satisfied by 1.4.2 and is still '
  + 'a range half of which nobody can read. A skill whose window is half unreadable is a skill '
  + 'whose publisher has to hear about it, so the operator refuses where the function would not.',
  { op: 'semver_satisfies', path: 'robot.runtime', value: '>=1.0.0 || garbage' })

// Composition.

predicateCase('and-all-hold', 'The ordinary conjunction.',
  { op: 'and', predicates: [
    { op: 'eq', path: 'robot.state.mode', value: 'idle' },
    { op: 'gte', path: 'robot.state.battery_percent', value: 20 },
  ] })
predicateCase('and-short-circuits-on-unsatisfied',
  'A conjunction whose first child is false stops there, so the refusal names a predicate that '
  + 'was actually false rather than the unreadable one behind it.',
  { op: 'and', predicates: [
    { op: 'eq', path: 'robot.state.mode', value: 'estop' },
    { op: 'gte', path: 'robot.state.absent', value: 1 },
  ] })
predicateCase('and-names-the-first-unevaluable',
  'And when the unreadable one comes first, that is what is named. Which predicate a refusal names '
  + 'must not depend on how an edition iterates.',
  { op: 'and', predicates: [
    { op: 'gte', path: 'robot.state.absent', value: 1 },
    { op: 'eq', path: 'robot.state.mode', value: 'estop' },
  ] })
predicateCase('and-empty', 'A skill declaring no preconditions is not a skill that fails them.',
  { op: 'and', predicates: [] })
predicateCase('or-holds-despite-an-unreadable-alternative',
  'An alternative that holds ends the question; the unreadable one beside it never mattered.',
  { op: 'or', predicates: [
    { op: 'gte', path: 'robot.state.absent', value: 1 },
    { op: 'eq', path: 'robot.state.mode', value: 'idle' },
  ] })
predicateCase('or-names-the-first-unevaluable',
  'With nothing holding, a disjunction containing something unreadable is not known to be false — '
  + 'only unread.',
  { op: 'or', predicates: [
    { op: 'eq', path: 'robot.state.mode', value: 'estop' },
    { op: 'gte', path: 'robot.state.absent', value: 1 },
  ] })
predicateCase('or-empty', 'No alternatives, nothing holds.',
  { op: 'or', predicates: [] })
predicateCase('not-over-unsatisfied', 'Negation of a decided answer is a decided answer.',
  { op: 'not', predicate: { op: 'eq', path: 'robot.state.mode', value: 'estop' } })
predicateCase('not-over-unevaluable-refuses',
  'The change that matters most. `not(eq(robot.state.mode, "estop"))` over a robot that has never '
  + 'reported a mode used to be **true**, because `!false` is true and `undefined === "estop"` is '
  + 'false. Ignorance does not negate into knowledge.',
  { op: 'not', predicate: { op: 'eq', path: 'robot.state.absent', value: 'estop' } })
predicateCase('not-over-exists-still-works',
  'And the idiom this must not break: `exists` answers about absence, so negating it is meaningful.',
  { op: 'not', predicate: { op: 'exists', path: 'robot.state.absent' } })
predicateCase('not-via-predicates-array', 'The first child of `predicates` is accepted as the operand.',
  { op: 'not', predicates: [{ op: 'eq', path: 'robot.state.mode', value: 'estop' }] })
predicateCase('not-with-no-operand-refuses',
  'A `not` with nothing to negate used to answer **true** — `!evaluate(undefined)` — which is a '
  + 'malformed rule passing.',
  { op: 'not' })

// Paths.

predicateCase('path-array-index', 'An array member is addressed by its index.',
  { op: 'eq', path: 'robot.guards.active.1', value: 'velocity' })
predicateCase('path-array-index-out-of-range', 'Past the end is nothing.',
  { op: 'eq', path: 'robot.guards.active.9', value: 'velocity' })
predicateCase('path-array-index-leading-zero',
  'Not a canonical index, and therefore not a member. JavaScript agrees: `["a"]["01"]` is undefined.',
  { op: 'exists', path: 'robot.guards.active.01' })
predicateCase('path-array-length-is-not-a-member',
  '`length` is a property of a JavaScript array and not a member of the document, so it does not '
  + 'resolve. An edition without JavaScript arrays could not reproduce it, and a path vocabulary '
  + 'one edition can read is not a path vocabulary.',
  { op: 'exists', path: 'robot.guards.active.length' })
predicateCase('path-prototype-is-not-a-member',
  'Nor is anything else off the prototype chain. Rules are operator-supplied rows, and a path they '
  + 'compose has no business reaching `constructor`.',
  { op: 'exists', path: 'robot.constructor' })
predicateCase('path-through-a-scalar', 'A path cannot continue through a string.',
  { op: 'exists', path: 'robot.tier.length' })
predicateCase('path-through-null', 'Nor through an explicit null.',
  { op: 'eq', path: 'robot.state.nothing.deep', value: 1 })
predicateCase('path-empty-segment', 'An empty segment names no member.',
  { op: 'exists', path: 'robot..tier' })
predicateCase('path-missing-entirely', 'A predicate with no path cannot read anything.',
  { op: 'eq', value: 'idle' })
predicateCase('path-not-a-string',
  'A `path` that is not a string used to reach `path.split` and **throw**, taking the whole '
  + 'compatibility check down with one bad rule row — in a module whose first paragraph says it is '
  + 'total. It is read defensively now.',
  { op: 'eq', path: 5, value: 'idle' })
predicateCase('path-empty-string', 'An empty path is no path.',
  { op: 'exists', path: '' })

// References.

predicateCase('ref-alias-single-key', '`{ path: … }` is accepted as an alias for `__ref`.',
  { op: 'includes_all', path: 'robot.capabilities', value: { path: 'skill.needs' } })
predicateCase('ref-alias-needs-to-be-alone',
  'With a second key it is an object literal, not a reference — and an object literal is never '
  + 'equal to anything, so this asks whether the capability list contains an object.',
  { op: 'includes_all', path: 'robot.capabilities', value: { path: 'skill.needs', note: 'x' } })
predicateCase('ref-to-a-missing-path-refuses', 'A reference that names nothing refuses, and the '
  + 'refusal names the reference rather than the left-hand path.',
  { op: 'eq', path: 'robot.state.mode', value: { __ref: 'skill.expected_mode' } })

// Malformed.

predicateCase('malformed-null', 'A null predicate is not a predicate.', null)
predicateCase('malformed-scalar', 'Nor is a number.', 42)
predicateCase('malformed-no-op', 'Nor is an object with no operator.', { path: 'robot.tier' })
predicateCase('malformed-op-not-a-string', 'Nor one whose operator is not a string.', { op: 5 })
predicateCase('unknown-op', 'An operator this edition does not implement is named, not guessed at.',
  { op: 'approximately', path: 'robot.state.battery_percent', value: 60 })
predicateCase('malformed-predicates-not-an-array',
  'A `predicates` that is not an array used to reach `.every` and throw. It is malformed — '
  + 'deliberately not "no children", because a conjunction over no children is *satisfied*, and '
  + 'turning a broken rule into a pass is the failure this whole change is about.',
  { op: 'and', predicates: 'nope' })

// A whole precondition, of the shape §54.2 describes.

predicateCase('skill-preconditions-hold',
  'What a `SkillManifest` precondition becomes: the guards that must be running, the calibration '
  + 'that must be valid, the battery that must be above reserve, the cell that must be closed.',
  { op: 'and', predicates: [
    { op: 'includes_all', path: 'robot.guards.active', value: ['workspace', 'collision'] },
    { op: 'eq', path: 'robot.calibration.kinematic.valid', value: true },
    { op: 'gte', path: 'robot.state.battery_percent', value: 20 },
    { op: 'eq', path: 'env.door_closed', value: true },
    { op: 'includes_all', path: 'robot.capabilities', value: { __ref: 'skill.needs' } },
    { op: 'semver_satisfies', path: 'robot.runtime', value: { __ref: 'skill.range' } },
  ] })
predicateCase('skill-preconditions-refused-by-calibration',
  'The same tree against the set that expired. §45 makes a lapsed calibration a fact rather than a '
  + 'warning, and this is where that fact stops a skill.',
  { op: 'and', predicates: [
    { op: 'eq', path: 'robot.calibration.torque.valid', value: true },
    { op: 'gte', path: 'robot.state.battery_percent', value: 20 },
  ] })
predicateCase('skill-preconditions-refused-by-an-undeclared-fact',
  'And the one §54.2 exists for: a precondition naming something this robot does not expose is '
  + 'refused with the path named, so the answer is "nobody declared a payload" rather than '
  + '"the payload is wrong".',
  { op: 'and', predicates: [
    { op: 'gte', path: 'robot.state.battery_percent', value: 20 },
    { op: 'lte', path: 'robot.state.payload_kg', value: 5 },
  ] })

// ── semver ───────────────────────────────────────────────────────────────────
//
// Range satisfaction is a contract because §54.2 made `semver_satisfies` an operator the robot
// evaluates. Validity is pinned beside satisfaction because the two answers are different facts:
// "outside the window" and "that is not a window" have different remedies.

function satisfiesCase(id: string, note: string, version: string, range: string): void {
  add('semver', 'satisfies', {
    id,
    note,
    input: { version, range },
    expected: canonical(shapeSatisfies(version, range, {
      satisfies: satisfies(version, range),
      validVersion: isValidVersion(version),
      validRange: isValidRange(range),
    })),
  })
}

satisfiesCase('exact-match', 'A bare version is an equality comparator.', '1.2.3', '1.2.3')
satisfiesCase('exact-mismatch', 'And it is exact.', '1.2.4', '1.2.3')
satisfiesCase('wildcard', 'A wildcard constrains nothing.', '9.9.9', '*')
satisfiesCase('any-word', '`any` is spelled out as well, case-insensitively.', '9.9.9', 'ANY')
satisfiesCase('empty-range', 'An empty range constrains nothing either.', '9.9.9', '')
satisfiesCase('caret-in-range', 'Caret holds the major.', '1.9.0', '^1.2.0')
satisfiesCase('caret-below', 'Below the base is outside it.', '1.1.0', '^1.2.0')
satisfiesCase('caret-major-bump', 'A major bump leaves it.', '2.0.0', '^1.2.0')
satisfiesCase('caret-zero-major', 'Under 1.0.0 the caret holds the minor instead.', '0.2.9', '^0.2.0')
satisfiesCase('caret-zero-major-bump', 'So a minor bump leaves it.', '0.3.0', '^0.2.0')
satisfiesCase('caret-zero-minor', 'And under 0.1.0 it holds the patch.', '0.0.5', '^0.0.5')
satisfiesCase('caret-zero-minor-bump', 'Which nothing else satisfies.', '0.0.6', '^0.0.5')
satisfiesCase('tilde', 'Tilde holds the minor.', '1.2.9', '~1.2.0')
satisfiesCase('tilde-minor-bump', 'A minor bump leaves it.', '1.3.0', '~1.2.0')
satisfiesCase('range-and', 'Space-separated comparators are a conjunction.', '1.5.0', '>=1.0.0 <2.0.0')
satisfiesCase('range-and-outside', 'Both must hold.', '2.1.0', '>=1.0.0 <2.0.0')
satisfiesCase('range-or', '`||` is a disjunction.', '3.1.0', '<2.0.0 || >=3.0.0')
satisfiesCase('range-or-gap', 'And the gap between them is outside.', '2.5.0', '<2.0.0 || >=3.0.0')
satisfiesCase('spaced-operator', 'The tokenizer splits on the operator, not on whitespace.', '1.3.0', '>= 1.2.0')
satisfiesCase('prerelease-below-release',
  'A prerelease is below its release, per semver §11.', '2.0.0-rc.1', '>=2.0.0')
satisfiesCase('prerelease-compared-to-prerelease',
  'And two prereleases order numerically where both identifiers are numbers.',
  '2.0.0-rc.2', '>2.0.0-rc.1')
satisfiesCase('unparseable-version', 'A version that does not parse satisfies nothing.', 'nonsense', '*')
satisfiesCase('two-segment-version', 'Two segments is not a version here.', '1.2', '>=1.0.0')
satisfiesCase('unreadable-range', 'A range with nothing in it satisfies nothing.', '1.2.3', 'garbage')
satisfiesCase('range-with-one-unreadable-alternative',
  'One unreadable alternative does not invalidate the readable ones — `satisfies` is true — and the '
  + 'range is still not wholly readable, which is the second answer this corpus pins.',
  '1.2.3', '>=1.0.0 || garbage')
satisfiesCase('range-with-a-leading-v',
  'The tokenizer is a scan rather than an anchored parse, so a `v` prefix is skipped over and the '
  + 'version behind it is the token. That is what the reference regex does; an edition that '
  + 'rejected it would be a stricter and different implementation.',
  '1.2.3', 'v1.2.3')
satisfiesCase('range-with-four-segments',
  'And the same scan takes the first three segments of a four-segment number.', '1.2.3', '1.2.3.4')
satisfiesCase('bare-pipes', 'Two empty alternatives read nothing.', '1.2.3', '||')

// The tokenizer, which is where an edition drifts.
//
// `satisfies` is a scan over a regex rather than an anchored parse, and every case below is a shape
// a hand-written parser gets subtly wrong in a different way. They are cheap and they are the ones
// that would otherwise be found by a robot refusing a skill its Hub page said it could run.

satisfiesCase('no-space-between-comparators',
  'The scan does not need whitespace to separate two comparators.', '1.5.0', '>=1.0.0<2.0.0')
satisfiesCase('no-space-around-pipes', 'Nor around the alternation.', '2.1.0', '^1.2.0||^2.0.0')
satisfiesCase('padded-range', 'A range is trimmed before anything else.', '1.5.0', '  ^1.2.0  ')
satisfiesCase('explicit-equals', 'An explicit `=` is the same as a bare version.', '1.2.3', '=1.2.3')
satisfiesCase('spaced-equals', 'With a space after it.', '1.2.3', '= 1.2.3')
satisfiesCase('reversed-arrow',
  'An operator that cannot start a match is retried as a shorter one and finally as none, which is '
  + 'what ordered alternation with backtracking does. `=>` is read as the `>` at its second '
  + 'character.', '1.5.0', '=>1.0.0')
satisfiesCase('less-than', 'Strictly below.', '1.2.2', '<1.2.3')
satisfiesCase('less-than-boundary', 'And the boundary is not below.', '1.2.3', '<1.2.3')
satisfiesCase('greater-than-or-equal-prerelease',
  'A comparator may itself carry a prerelease.', '1.2.3', '>=1.2.3-rc.1')
satisfiesCase('caret-with-prerelease-base',
  'And so may a caret base.', '1.2.3', '^1.2.3-rc.1')
satisfiesCase('tilde-zero-zero', 'Tilde on a 0.0.x base still holds the minor.', '0.0.9', '~0.0.1')
satisfiesCase('contradictory-conjunction',
  'Two exact comparators in one alternative can never both hold. Nothing satisfies it, and it is '
  + 'still a readable range — which is the distinction this corpus exists to keep.',
  '1.2.3', '1.2.3 1.2.4')
satisfiesCase('empty-first-alternative',
  'An empty alternative reads nothing, and the readable one beside it still decides.',
  '1.2.3', '|| 1.2.3')
satisfiesCase('empty-last-alternative', 'And the same at the other end.', '1.2.3', '1.2.3 ||')
satisfiesCase('range-of-letters', 'No token, no comparator.', '1.2.3', 'x.y.z')
satisfiesCase('two-segment-range', 'A two-segment number is not a version and yields no token.',
  '1.2.3', '>=1.2')
satisfiesCase('nbsp-before-the-version',
  'A non-breaking space is whitespace to JavaScript, so the tokenizer steps over it. An edition '
  + 'that reached for the whitespace set of its own language would answer differently, once, in '
  + 'the field, to somebody who could not explain it.',
  '1.2.3', '>=\u00A01.2.3')
satisfiesCase('bom-between-operator-and-version',
  'And so is a byte-order mark — the character where JavaScript and Rust disagree by default: '
  + 'trim() removes it and char::is_whitespace does not.',
  '1.2.3', '>=\uFEFF1.2.3')
satisfiesCase('prerelease-does-not-satisfy-a-release-range',
  'The trap semver §11 exists for: 2.0.0-rc.1 is below 2.0.0, so a range asking for 2.x does not '
  + 'admit its own release candidate.', '2.0.0-rc.1', '^2.0.0')

function compareCase(id: string, note: string, a: string, b: string): void {
  add('semver', 'compare', {
    id,
    note,
    input: { a, b },
    expected: canonical({
      compare: compareVersions(a, b),
      a: shapeVersion(parseVersion(a)),
      b: shapeVersion(parseVersion(b)),
    }),
  })
}

compareCase('equal', 'Identical versions.', '1.2.3', '1.2.3')
compareCase('major', 'Major dominates.', '2.0.0', '1.9.9')
compareCase('minor-numeric-not-lexical',
  'Ten is after two, which is the comparison a string sort gets wrong.', '1.2.0', '1.10.0')
compareCase('patch', 'And the patch.', '1.2.3', '1.2.4')
compareCase('release-outranks-prerelease', 'A release outranks its own prerelease.', '1.0.0', '1.0.0-rc.1')
compareCase('prerelease-numeric', 'Numeric identifiers compare as numbers.', '1.0.0-2', '1.0.0-10')
compareCase('prerelease-alpha', 'Alphanumeric identifiers compare as strings.', '1.0.0-alpha', '1.0.0-beta')
compareCase('prerelease-numeric-before-alpha',
  'A numeric identifier is below an alphanumeric one.', '1.0.0-1', '1.0.0-alpha')
compareCase('prerelease-length', 'A longer identifier list wins a tie on the shared prefix.',
  '1.0.0-alpha.1', '1.0.0-alpha')
compareCase('unparseable-left',
  'Unparseable means **equal**, which is the reference and is not what this would be if it were '
  + 'being designed now. Reproduced because a sort that ordered a malformed list differently in two '
  + 'editions is worse than a comparator with a documented wart.',
  'nonsense', '1.0.0')
compareCase('unparseable-both', 'And the same when neither side parses.', '', 'x')
compareCase('trimmed', 'Both sides are trimmed before parsing.', '  1.2.3  ', '1.2.3')
compareCase('leading-zeros', 'Numeric components are numbers, so leading zeros do not change one.',
  '1.02.3', '1.2.3')
compareCase('large-major',
  'Components are read with `Number()`, so a value beyond an integer\'s exact range rounds rather '
  + 'than failing to parse. Both editions must round it the same way.',
  '9007199254740993.0.0', '9007199254740992.0.0')

// ── skill ────────────────────────────────────────────────────────────────────
//
// §54's four pure decisions: may this skill start, did it succeed, when is it stopped, and is its
// call graph installable. The lifecycle that will ask them needs leases and §27's state machine and
// does not exist, which is exactly why the answers are frozen now — so that when something does
// loop, it is not also inventing the rules.

/** The post-state and the live state are the same shape; §54.3 evaluates over the later one. */
const CELL = {
  robot: {
    tier: 'host',
    state: { battery_percent: 62, gripper: 'open', mode: 'idle' },
    guards: { active: ['workspace', 'collision'] },
    calibration: { kinematic: { valid: true }, torque: { valid: false } },
  },
  env: { door_closed: true, fixture: 'jig-a' },
}

const AFTER = {
  robot: {
    state: { battery_percent: 58, gripper: 'closed', mode: 'idle' },
    grasp: { holding: true, slipped: false },
  },
}

function preconditionCase(
  id: string, note: string,
  published: unknown, operator: unknown, context: unknown, phase: Phase = 'entry',
): void {
  const clauses = resolveClauses(readClauses(published), readClauses(operator))
  add('skill', 'preconditions', {
    id,
    note,
    input: { published, operator, context, phase },
    expected: canonical(shapePreconditions(evaluatePreconditions(clauses, context, phase))),
  })
}

preconditionCase('prose-refuses',
  'What every skill published before §54.2 carries. Three sentences, no predicates, and prose does '
  + 'not stop a robot — so all three are unenforceable and the skill is refused. The report is the '
  + 'publisher’s to-do list rather than one verdict, which is the point of listing them.',
  ['The gripper is empty', 'The cell door is closed', 'The workpiece is in the jig'], [], CELL)

preconditionCase('predicates-hold',
  'The same three preconditions, expressed as predicates. This is what §54.2 asks a publisher for.',
  [
    { statement: 'The gripper is empty', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } },
    { statement: 'The cell door is closed', predicate: { op: 'eq', path: 'env.door_closed', value: true } },
    { statement: 'Kinematic calibration is valid', predicate: { op: 'eq', path: 'robot.calibration.kinematic.valid', value: true } },
  ], [], CELL)

preconditionCase('a-precondition-that-is-false',
  'The ordinary refusal: the world is not ready. The remedy is to change the world.',
  [
    { statement: 'Torque calibration is valid', predicate: { op: 'eq', path: 'robot.calibration.torque.valid', value: true } },
  ], [], CELL)

preconditionCase('a-precondition-that-cannot-be-evaluated',
  'The refusal §54.2 exists for, and a different sentence with a different remedy: this manifest '
  + 'names something this robot does not expose, so the path is named and the publisher is the one '
  + 'who has to hear about it.',
  [
    { statement: 'Payload is under 5 kg', predicate: { op: 'lte', path: 'robot.state.payload_kg', value: 5 } },
  ], [], CELL)

preconditionCase('every-clause-is-reported-not-just-the-first',
  'Four clauses, three of them wrong in three different ways. An operator fixing them one attempt '
  + 'at a time is the failure `admitPackage` already refuses to cause, so all four are evaluated '
  + 'and `refused` names the first.',
  [
    { statement: 'Battery above reserve', predicate: { op: 'gte', path: 'robot.state.battery_percent', value: 20 } },
    { statement: 'Torque calibration is valid', predicate: { op: 'eq', path: 'robot.calibration.torque.valid', value: true } },
    { statement: 'Payload is under 5 kg', predicate: { op: 'lte', path: 'robot.state.payload_kg', value: 5 } },
    'The workpiece is in the jig',
  ], [], CELL)

preconditionCase('the-operator-adds-one',
  '§54.2’s third rule: a site can require "door closed" on a skill whose publisher never '
  + 'imagined that cell. The site’s clause is appended, carries `operator` as its origin, and '
  + 'is evaluated exactly like the publisher’s.',
  [{ statement: 'Gripper open', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } }],
  [{ statement: 'Fixture B is loaded', predicate: { op: 'eq', path: 'env.fixture', value: 'jig-b' } }],
  CELL)

preconditionCase('the-skill-cannot-remove-one',
  'The other half of the same rule, and the reason it needs no enforcement code: the operator’s '
  + 'list is only ever appended, so the result is never shorter than the publisher’s. An empty '
  + 'operator list changes nothing, and there is no spelling of one that could.',
  [
    { statement: 'Gripper open', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } },
    { statement: 'Door closed', predicate: { op: 'eq', path: 'env.door_closed', value: true } },
  ], [], CELL)

preconditionCase('nothing-declared',
  'A skill with no preconditions is not a skill that fails them.', [], [], CELL)

const HOLDING = [
  { statement: 'Gripper open', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } },
  {
    statement: 'Battery above reserve',
    predicate: { op: 'gte', path: 'robot.state.battery_percent', value: 20 },
    hold: true,
  },
]

preconditionCase('hold-subset-at-entry',
  '§54.2’s second rule, first half: at entry every clause is evaluated, marked or not.',
  HOLDING, [], CELL, 'entry')

preconditionCase('hold-subset-every-cycle',
  'And the second half: the cycle re-check evaluates only the marked one. "Battery dropping below '
  + 'reserve mid-skill should stop the skill, not be discovered at the end" — and the gripper, '
  + 'which the skill itself closed on purpose, is not re-asked.',
  HOLDING, [], CELL, 'hold')

preconditionCase('hold-with-nothing-marked',
  'A skill whose author marked nothing has nothing re-checked, and the cycle permits it. That is a '
  + 'publisher’s decision rather than a gap: the entry check still ran.',
  [{ statement: 'Gripper open', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } }],
  [], CELL, 'hold')

preconditionCase('hold-loses-the-battery',
  'The scenario §54.2 describes, at the moment it matters: the same marked clause, one cycle after '
  + 'the battery fell through reserve.',
  HOLDING, [], {
    ...CELL,
    robot: { ...CELL.robot, state: { ...CELL.robot.state, battery_percent: 12 } },
  }, 'hold')

preconditionCase('a-predicate-that-is-a-number',
  'Not a predicate. The reference reaches it through `value && typeof value === "object"`, so a '
  + 'number there means the clause is unenforceable rather than malformed — the author wrote no '
  + 'predicate, they did not write a broken one.',
  [{ statement: 'Something', predicate: 5 }], [], CELL)

preconditionCase('a-predicate-that-is-an-array',
  'And the case on the other side of that line: an array *is* an object to JavaScript, so it '
  + 'reaches the evaluator, which answers malformed. A broken predicate and an absent one are '
  + 'different reports because they have different authors.',
  [{ statement: 'Something', predicate: ['op'] }], [], CELL)

preconditionCase('a-clause-with-no-statement',
  'The sentence is what an operator reads, and a clause without one is still evaluated — reporting '
  + 'nothing would hide a refusal.',
  [{ predicate: { op: 'eq', path: 'robot.state.mode', value: 'estop' } }], [], CELL)

preconditionCase('entries-that-are-not-clauses',
  'A null and a number in the list are skipped rather than invented into empty clauses.',
  [null, 42, 'Prose survives'], [], CELL)

preconditionCase('preconditions-that-are-not-a-list',
  'Neither is a string a list of clauses.', 'the door is closed', [], CELL)

// ── §54.3 ────────────────────────────────────────────────────────────────────

function successCase(id: string, note: string, criteria: unknown, postState: unknown): void {
  const clauses = resolveClauses(readClauses(criteria))
  add('skill', 'success', {
    id,
    note,
    input: { criteria, post_state: postState },
    expected: canonical(shapeSuccess(judgeSuccess(clauses, postState))),
  })
}

successCase('every-criterion-held',
  'Judged by the OS over post-state, not by the skill. A skill that judges its own success is a '
  + 'skill that always succeeds.',
  [
    { statement: 'The gripper is closed', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'closed' } },
    { statement: 'The part did not slip', predicate: { op: 'eq', path: 'robot.grasp.slipped', value: false } },
  ], AFTER)

successCase('a-criterion-was-false',
  'One criterion definitely false is a failure.',
  [{ statement: 'The gripper is open', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } }],
  AFTER)

successCase('a-false-criterion-outranks-an-unreadable-one',
  'The order the three answers are decided in, and the case that pins it: the unreadable criterion '
  + 'comes **first** in the list and the answer is still `failure`. Evidence of failure beats '
  + 'absence of evidence, and an edition that took the first non-satisfied clause would say '
  + '`unlabeled` here.',
  [
    { statement: 'The cup is upright', predicate: { op: 'eq', path: 'robot.perception.cup_upright', value: true } },
    { statement: 'The gripper is open', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'open' } },
  ], AFTER)

successCase('an-unreadable-criterion-is-unlabeled',
  'Not a failure. §54.3: "Honest ignorance beats an invented pass" — and calling an unmeasured run '
  + 'a failure poisons a dataset exactly as thoroughly as calling it a success.',
  [
    { statement: 'The gripper is closed', predicate: { op: 'eq', path: 'robot.state.gripper', value: 'closed' } },
    { statement: 'The cup is upright', predicate: { op: 'eq', path: 'robot.perception.cup_upright', value: true } },
  ], AFTER)

successCase('a-prose-criterion-is-unlabeled',
  '§54.3’s own example. "The cup is upright" needs perception, is genuinely not '
  + 'machine-evaluable, and is `unlabeled` rather than either verdict.',
  [
    { statement: 'The part did not slip', predicate: { op: 'eq', path: 'robot.grasp.slipped', value: false } },
    'The cup is upright',
  ], AFTER)

successCase('no-criteria-is-unlabeled',
  'Nothing was measured. A skill that scores itself by declaring nothing is the same gaming §54.3 '
  + 'exists to stop, so the absence of criteria is not a pass.',
  [], AFTER)

// ── §54.4 ────────────────────────────────────────────────────────────────────

function timeoutCase(
  id: string, note: string, expected: number | null, factor?: unknown,
): void {
  add('skill', 'timeout', {
    id,
    note,
    input: { expected_duration_seconds: expected, overrun_factor: factor ?? null },
    expected: canonical(shapeDeadline(
      factor === undefined ? skillDeadline(expected) : skillDeadline(expected, factor as number),
    )),
  })
}

timeoutCase('declared-duration', 'Thirty seconds at the default factor of two.', 30)
timeoutCase('explicit-factor', 'A site that will tolerate half again as long.', 30, 1.5)
timeoutCase('factor-below-one-is-refused',
  'A factor under one would stop a skill before its own estimate, which is not an overrun factor. '
  + 'The default is used rather than the number somebody typed.', 30, 0.5)
timeoutCase('factor-that-is-not-a-number',
  'A factor written as a string is not a factor. `NaN` and the infinities are refused the same way '
  + 'and are not here, because JSON cannot hold one — they are in the unit suites of both editions '
  + 'instead, which is where a value the corpus format cannot express has to be checked.',
  30, '1.5')
timeoutCase('no-declared-duration',
  'The interesting one. §54.4 says the OS enforces a timeout and does not say what to do about a '
  + 'skill that declares no duration, so this reports that it cannot be enforced rather than '
  + 'inventing a ceiling. Refusing an unbounded skill and permitting one are both defensible, and '
  + 'the decision belongs where policy is made.', null)
timeoutCase('zero-duration', 'Zero is not a duration.', 0)
timeoutCase('negative-duration', 'Nor is a negative one.', -5)
timeoutCase('rounding',
  'Milliseconds are integers. Half rounds up, which is JavaScript’s `Math.round` and is why '
  + 'the sign of both operands is checked before the multiplication rather than after it.',
  0.0025, 1)
timeoutCase('long-skill', 'A twenty-minute skill, which is still bounded.', 1200)

// ── §54.5 ────────────────────────────────────────────────────────────────────

function compositionCase(
  id: string, note: string, root: string, calls: unknown, maxDepth?: number,
): void {
  const parsed = readCalls(calls)
  add('skill', 'composition', {
    id,
    note,
    input: { root, calls, max_depth: maxDepth ?? null },
    expected: canonical(shapeComposition(
      maxDepth === undefined
        ? validateComposition(root, parsed)
        : validateComposition(root, parsed, maxDepth),
    )),
  })
}

compositionCase('within-the-limit',
  'Four skills deep, which the default permits: the root counts as depth one.',
  'pick', [
    { from: 'pick', to: 'approach' },
    { from: 'approach', to: 'align' },
    { from: 'align', to: 'servo' },
  ])

compositionCase('one-too-deep',
  'Five is not four, and the fault names the whole chain rather than the last link — an operator '
  + 'needs to see which composition got out of hand.',
  'pick', [
    { from: 'pick', to: 'approach' },
    { from: 'approach', to: 'align' },
    { from: 'align', to: 'servo' },
    { from: 'servo', to: 'nudge' },
  ])

compositionCase('a-skill-that-calls-itself',
  'The simplest cycle, and it is refused at install rather than discovered by a robot that has '
  + 'already taken a lease and started moving.',
  'pick', [{ from: 'pick', to: 'pick' }])

compositionCase('an-indirect-cycle',
  'Three skills round a loop. The repeated element is the last entry of the path, so the chain '
  + 'reads as the walk that found it.',
  'pick', [
    { from: 'pick', to: 'approach' },
    { from: 'approach', to: 'retry' },
    { from: 'retry', to: 'pick' },
  ])

compositionCase('a-diamond-is-not-a-cycle',
  'Two paths reaching one skill is a shared child, not a loop. An edition that marked nodes '
  + 'globally visited rather than tracking the current chain would refuse this, and it is legal.',
  'pick', [
    { from: 'pick', to: 'left' },
    { from: 'pick', to: 'right' },
    { from: 'left', to: 'grasp' },
    { from: 'right', to: 'grasp' },
  ])

compositionCase('declaration-order-decides-which-fault',
  'A graph with both faults in it. Depth-first in declaration order reaches the cycle first, and '
  + 'pinning that is the point: an edition iterating its edges differently would refuse the same '
  + 'graph and blame a different skill, which is the half an operator reads.',
  'pick', [
    { from: 'pick', to: 'loop' },
    { from: 'pick', to: 'deep1' },
    { from: 'loop', to: 'pick' },
    { from: 'deep1', to: 'deep2' },
    { from: 'deep2', to: 'deep3' },
    { from: 'deep3', to: 'deep4' },
  ])

compositionCase('edges-that-do-not-start-at-the-root',
  'A graph the root cannot reach is not this skill’s problem. Only what it can call is walked.',
  'pick', [
    { from: 'place', to: 'place' },
    { from: 'pick', to: 'approach' },
  ])

compositionCase('no-calls-at-all', 'A skill that calls nothing composes trivially.', 'pick', [])

compositionCase('a-depth-limit-of-one',
  'The root and nothing else, which is a site that has switched composition off.',
  'pick', [{ from: 'pick', to: 'approach' }], 1)

compositionCase('a-depth-limit-of-zero-is-not-a-limit',
  'Zero would refuse the root itself, which is not a composition rule. The default is used.',
  'pick', [{ from: 'pick', to: 'approach' }], 0)

compositionCase('malformed-edges-are-skipped',
  'An edge whose ends are not names is not an edge. Both editions discard it in `readCalls`, at '
  + 'the same boundary, so this case is testing one function rather than two.',
  'pick', [
    { from: 5, to: 'approach' },
    { from: 'pick' },
    null,
    { from: 'pick', to: 'pick' },
  ])

// ── cell ─────────────────────────────────────────────────────────────────────
//
// §55.2's cell descriptor: the physical counterpart of an environment, living in the operator
// profile rather than in the repository catalogue because a factory layout is not intellectual
// property and safety-relevant volumes do not belong behind a fork-and-star workflow.
//
// It does two jobs, and both are pinned: it supplies `env.*` to §54.2's precondition context, and
// it supplies allowed volumes to the workspace guard as one layer of §38.2's stack.

const BENCH: Volume = {
  frame: 'right_arm.tool0',
  shape: { kind: 'box', min: [-0.6, -0.6, 0.0], max: [0.6, 0.6, 1.2] },
}

function cell(overrides: Partial<CellDescriptor> = {}): CellDescriptor {
  return {
    name: 'bay-3',
    facts: { door_closed: true, fixture: 'jig-a', ambient_c: 21.5 },
    allowed: [BENCH],
    fixtures: [{ name: 'jig-a' }, { name: 'bin', volume: BENCH }],
    parking_pose: [{ channel: 'J1', position: 0 }, { channel: 'J2', position: -1.2 }],
    ...overrides,
  }
}

function cellCase(id: string, note: string, subject: CellDescriptor): void {
  add('cell', 'validate', {
    id,
    note,
    input: { cell: subject },
    expected: canonical(shapeCellFaults(validateCell(subject))),
  })
}

cellCase('clean', 'A cell somebody measured: a name, three facts, a volume, two fixtures and a '
  + 'parking pose.', cell())

cellCase('unnamed', 'A cell has no identity but its name.', cell({ name: '' }))
cellCase('name-of-spaces', 'And whitespace is not a name.', cell({ name: '   ' }))

cellCase('a-fact-key-with-a-dot-in-it',
  'The check that could not have existed before §54.2. A precondition addresses this as '
  + '`env.door.closed`, which is the path to `closed` inside an object called `door` — so the fact '
  + 'is unreachable, and it is the obvious way to write a hierarchical key for anybody who has not '
  + 'read `readPath`.',
  cell({ facts: { 'door.closed': true } }))

cellCase('a-fact-key-with-surrounding-space',
  'A path segment is the key exactly. Trimming it here would make the document and the path '
  + 'disagree about what the fact is called.',
  cell({ facts: { ' door_closed ': true } }))

cellCase('an-empty-fact-key', 'A path cannot name nothing.', cell({ facts: { '': true } }))

cellCase('a-fact-that-shadows-the-cell-name',
  '`env.cell` is derived from the cell itself. A fact of that name would be read by nobody and '
  + 'would make a precondition true for the wrong reason.',
  cell({ facts: { cell: 'somewhere-else' } }))

cellCase('a-fact-that-shadows-the-fixture-list',
  'And the same for the other derived one.', cell({ facts: { fixtures: [] } }))

cellCase('a-volume-with-no-frame',
  'A volume constrains a frame, and the workspace guard would have nothing to check this against.',
  cell({ allowed: [{ frame: '', shape: BENCH.shape }] }))

cellCase('an-inverted-box',
  'A box whose minimum exceeds its maximum on any axis is empty, and an empty *allowed* volume is '
  + 'a cell the robot may never be in. That is a configuration mistake rather than a lock-out to '
  + 'enforce silently.',
  cell({ allowed: [{ frame: 'right_arm.tool0', shape: { kind: 'box', min: [1, 0, 0], max: [0, 1, 1] } }] }))

cellCase('a-sphere-with-no-radius',
  'Zero is not a radius. A sphere nothing can be inside forbids the frame it names.',
  cell({ allowed: [{ frame: 'right_arm.tool0', shape: { kind: 'sphere', centre: [0, 0, 0], radius_m: 0 } }] }))

cellCase('a-shape-this-edition-does-not-know',
  'A cylinder is not in the vocabulary, and a volume nobody can evaluate is reported rather than '
  + 'quietly dropped — a workspace layer silently missing the volume an operator drew is a robot '
  + 'with more room than anybody agreed to.',
  cell({ allowed: [{ frame: 'right_arm.tool0', shape: { kind: 'cylinder', radius_m: 1 } } as never] }))

cellCase('an-unnamed-fixture',
  'A fixture a precondition cannot name is a fixture the OS cannot check for.',
  cell({ fixtures: [{ name: '' }] }))

cellCase('two-fixtures-with-one-name',
  'A precondition asking for it would be asking about whichever the reader happened to reach.',
  cell({ fixtures: [{ name: 'jig-a' }, { name: 'jig-a' }] }))

cellCase('a-fixture-measured-wrongly',
  'A fixture may be named without being measured; it may not be measured wrongly.',
  cell({ fixtures: [{ name: 'bin', volume: { frame: 'base', shape: { kind: 'sphere', centre: [0, 0, 0], radius_m: -1 } } }] }))

cellCase('a-parking-entry-with-no-channel',
  'A parking pose is a command, and a command names its channel.',
  cell({ parking_pose: [{ channel: '', position: 0 }] }))

cellCase('one-channel-parked-twice',
  'Which position the robot drives to would depend on the order a reader happened to walk the list.',
  cell({ parking_pose: [{ channel: 'J1', position: 0 }, { channel: 'J1', position: 1 }] }))

cellCase('several-faults-at-once',
  'Every fault, not the first. An operator is editing this by hand, and `admitPackage`’s reason '
  + 'applies unchanged: fixing four problems one deployment at a time is how people end up giving '
  + 'up. Fact keys are reported in sorted order, which is the only order a parsed object has in '
  + 'every edition.',
  {
    name: '',
    facts: { 'z.bad': 1, 'a.bad': 2, cell: 'x' },
    allowed: [{ frame: '', shape: { kind: 'box', min: [1, 1, 1], max: [0, 0, 0] } }],
    fixtures: [{ name: '' }],
    parking_pose: [{ channel: 'J1', position: 0 }, { channel: 'J1', position: 2 }],
  })

cellCase('an-empty-cell',
  'A cell with a name and nothing else is a cell nobody has measured yet, and that is not a fault.',
  { name: 'bay-4' })

function cellContextCase(id: string, note: string, subject: CellDescriptor): void {
  add('cell', 'context', {
    id,
    note,
    input: { cell: subject },
    expected: canonical({
      env: cellEnvironment(subject),
      ...shapeCellLayer(cellWorkspaceLayer(subject)) as Record<string, unknown>,
    }),
  })
}

cellContextCase('facts-and-volumes',
  'Both projections of one document: the facts a precondition reads as `env.*`, and the layer the '
  + 'workspace guard reads. Pinned together because they are two views of one thing and an edition '
  + 'that agreed about one could disagree about the other.',
  cell())

cellContextCase('derived-facts-are-always-there',
  '`env.cell` and `env.fixtures` come from the cell rather than from its facts, so a precondition '
  + 'can rely on them on a cell whose operator declared nothing.',
  { name: 'bay-4', fixtures: [{ name: 'jig-a' }, { name: 'bin' }] })

cellContextCase('an-unusable-key-does-not-reach-the-context',
  'The fault is reported and the fact is dropped. Passing it through would put a key in the '
  + 'context that no path can name, which is a fact that exists and cannot be read — worse than '
  + 'one that is missing, because it looks present in the document.',
  { name: 'bay-3', facts: { 'door.closed': true, door_open: false } })

cellContextCase('a-shadowing-fact-does-not-win',
  'The derived fact is the one a precondition gets.',
  { name: 'bay-3', facts: { cell: 'somewhere-else' } })

cellContextCase('a-cell-that-draws-nothing-adds-no-layer',
  'Not the same as a cell that allows nothing. An empty layer would forbid every frame it named, '
  + 'and a site that has not yet measured its cell would find the robot refusing to move with no '
  + 'volume anywhere to explain why.',
  { name: 'bay-4' })

cellContextCase('an-unusable-volume-is-not-in-the-layer',
  'It is reported by `validateCell` and it is not handed to the guard, because a shape the kernel '
  + 'cannot evaluate is not a constraint. The usable one beside it still is.',
  {
    name: 'bay-3',
    allowed: [BENCH, { frame: 'base', shape: { kind: 'sphere', centre: [0, 0, 0], radius_m: 0 } }],
  })

cellContextCase('every-volume-unusable-means-no-layer',
  'And when nothing survives, there is no layer rather than an empty one — the same distinction '
  + 'again, arrived at from the other direction.',
  { name: 'bay-3', allowed: [{ frame: '', shape: BENCH.shape }] })

// ── opstate ──────────────────────────────────────────────────────────────────
//
// §27's operating state machine, extended by §48.1. Four sections read this machine and until this
// session none of them could: `post.rs` returned the state as a string, §49.1's tone compared two
// strings, §33.4's barrier took a boolean a caller supplied, and §54.4's lifecycle waited.
//
// Three files, because the machine has three separable ways to be wrong and an edition could agree
// about one while disagreeing about another:
//
//  * `evaluate`   — where the machine goes, and *why*. The reason is pinned beside the state
//                   because two rules produce `hold` and only the reason distinguishes them.
//  * `properties` — what a state *means* to §33.4, §49.1 and §27. No evaluation case can see a
//                   divergence here, because these are read off the state rather than produced.
//  * `warning`    — all forty-nine ordered pairs of §49.1's tone, in full. A predicate whose
//                   failure mode is silence does not get sampled.

function opConditions(overrides: OpConditions = {}): OpConditions {
  return {
    post: 'passed',
    homing: 'complete',
    kernel: 'live',
    rhal: 'present',
    policy: 'ready',
    ...overrides,
  }
}

function opCase(
  id: string,
  note: string,
  previous: OperatingState,
  conditions: OpConditions,
): void {
  add('opstate', 'evaluate', {
    id,
    note,
    input: { previous, conditions },
    expected: canonical(shapeTransition(evaluateState(previous, conditions))),
  })
}

// The nominal robot, and the one nobody has looked at.

opCase('nominal', 'Booted, homed, kernel live, body present, policy holding its lease. Every rule '
  + 'above `nominal` declined, which is the strongest statement this corpus makes about the '
  + 'ordering: the reason names the last rule standing.',
  'hold', opConditions())

opCase('observed-nothing', 'The most important case in this file. A caller that has observed '
  + 'nothing has established nothing, and every default is the least capable reading — so an empty '
  + 'conditions object is a robot in SAFE with a lost RHAL, not a robot licensed to run by an '
  + 'object literal.',
  'post', {})

opCase('observed-nothing-from-running', 'The same conditions from the other end. Nothing about '
  + 'having been running survives an observation that establishes nothing.',
  'running', {})

// §27's SAFE row, both halves.

opCase('rhal-lost', 'The kernel lost the body. First rule, because it is the condition under which '
  + 'nothing else can be known: no observed joint state, no estop reading, no guard that can '
  + 'evaluate. It outranks a ready policy and a passing self-test.',
  'running', opConditions({ rhal: 'lost' }))

opCase('post-failed', '§48.2: SAFE, not DEGRADED. Sticky, needs a human, "because a robot that '
  + 'failed its own self-test should not be talked into running by a reconcile loop".',
  'post', opConditions({ post: 'failed' }))

opCase('stop-complete', 'The ramp finished. §27 puts this in the SAFE row beside a lost RHAL.',
  'stopping', opConditions({ stop: 'operator', stop_settled: true }))

// §27's four stop causes, and no fifth.

opCase('stopping-watchdog', '§46 fired. The cause reaches the reason, because "the robot is '
  + 'stopping" and "the watchdog stopped it" send an operator to different places.',
  'running', opConditions({ stop: 'watchdog' }))

opCase('stopping-proximity', 'Something came inside the stop distance.',
  'running', opConditions({ stop: 'proximity' }))

opCase('stopping-balance', 'The balance guard.',
  'running', opConditions({ stop: 'balance' }))

opCase('stopping-operator', 'A person asked.',
  'running', opConditions({ stop: 'operator' }))

opCase('stop-during-post', 'An operator stop is honoured whatever the robot is doing, including '
  + 'booting — which is why the stop rule sits above the POST rules rather than below them.',
  'post', opConditions({ post: 'incomplete', stop: 'operator' }))

opCase('settled-with-no-stop', '`stop_settled` is read only when a stop is commanded. A caller '
  + 'that left it set between stops would otherwise pin every robot on a fleet in SAFE.',
  'running', opConditions({ stop_settled: true }))

// §48.

opCase('post-incomplete', '§48.4’s third answer, and the distinction the section spends its '
  + 'length on. Nothing failed and nothing was established, so this robot does not need a human — '
  + 'it needs the rest of the operating system. `post`, not `safe`.',
  'post', opConditions({ post: 'incomplete' }))

opCase('post-unstated', 'A conditions document that does not mention the self-test is a self-test '
  + 'that has not run, which is `incomplete` rather than a pass.',
  'post', { homing: 'complete', kernel: 'live', rhal: 'present', policy: 'ready' })

opCase('homing-pending', '§48.1: joint positions are unknown, forward kinematics is meaningless, '
  + 'every pose-dependent guard is unavailable. Homing is the only motion permitted (§48.3).',
  'post', opConditions({ homing: 'pending' }))

opCase('homing-not-required', '"A body that needs no homing declares so and the state is skipped."',
  'post', opConditions({ homing: 'not_required' }))

opCase('homing-unstated', 'And a body that has declared nothing is a body whose joint positions '
  + 'are unknown, so silence reads as `pending` rather than as `not_required`.',
  'post', { post: 'passed', kernel: 'live', rhal: 'present', policy: 'ready' })

// §27's HOLD row, and the kernel.

opCase('kernel-down', '§27: "a robot with a policy and no kernel must not exist, so the runtime '
  + 'refuses to start if the kernel is not live". Refusing to start is HOLD — the safe resting '
  + 'state — rather than SAFE: nothing has gone wrong, and a sticky state needing a human would be '
  + 'the wrong answer for a kernel that is merely still starting.',
  'hold', opConditions({ kernel: 'down' }))

opCase('kernel-down-outranks-a-ready-policy', 'A policy ready to command a body with no kernel '
  + 'between it and the joints is the one arrangement §27 refuses outright, and it is refused from '
  + '`running` as readily as from `hold`.',
  'running', opConditions({ kernel: 'down', guards_unavailable: true }))

opCase('guard-hold', '§27’s HOLD row, first half. A guard asking for a hold outranks a policy '
  + 'that would like to run, which is the whole point of a guard.',
  'running', opConditions({ guard_hold: true }))

opCase('no-policy', '§27’s HOLD row, second half. There is nothing to run.',
  'running', opConditions({ policy: 'none' }))

opCase('policy-not-ready', 'The same state and a completely different situation: there is '
  + 'something to run and it has no lease or no plan. Only this one has a remedy — §25.2’s '
  + 'arbitration table is where an operator goes to find out who else is holding the arm — which is '
  + 'why the reason is split and the state is not.',
  'hold', opConditions({ policy: 'held' }))

// §27's DEGRADED row, which is a running state.

opCase('guards-unavailable', 'A guard is unavailable under a ready policy. §45.5 is the usual '
  + 'source, and it is an ordinary condition rather than an exotic one.',
  'running', opConditions({ guards_unavailable: true }))

opCase('rhal-degraded', 'The body reports RHAL_DEGRADED and is still answering. Distinct from '
  + '`lost`, which is the whole of SAFE.',
  'running', opConditions({ rhal: 'degraded' }))

opCase('degraded-prefers-the-specific-reason', 'Both conditions hold. The unavailable guard is '
  + 'reported, because it names something an operator can look up and "the body says it is '
  + 'impaired" does not.',
  'running', opConditions({ guards_unavailable: true, rhal: 'degraded' }))

opCase('an-unavailable-guard-with-no-policy-is-hold', 'The decision the ordering encodes, and the '
  + 'one place this machine could most plausibly have been built differently. §27’s DEGRADED row '
  + 'says the robot "**runs**, reports, and refuses any package whose grants depend on the failed '
  + 'guard" — so `degraded` is the impaired *running* state, not a third resting one. Calling this '
  + 'robot degraded would tell a console it is running while it stands still.',
  'hold', opConditions({ guards_unavailable: true, policy: 'none' }))

opCase('entering-degraded-sounds-the-tone', 'The bug the string comparison froze as correct. '
  + '`hold → degraded` is a policy beginning to command a body; it is not a transition into '
  + '`running`, and the old predicate compared against that literal. A robot with a lapsed '
  + 'calibration would have begun autonomous motion in silence.',
  'hold', opConditions({ guards_unavailable: true }))

// §27's stickiness.

opCase('safe-is-sticky', '"A robot that recovers itself from a safety stop is a robot that resumes '
  + 'moving while somebody is standing next to it working out what happened." Every condition is '
  + 'nominal and it does not move.',
  'safe', opConditions())

opCase('safe-cleared', 'The explicit human action §27 requires, by name — because §13 takes the '
  + 'transition and "somebody cleared the safety stop" is worth exactly as much as the name of the '
  + 'somebody. The tone sounds, which is §49.1’s "resumption after a stop".',
  'safe', opConditions({ cleared_by: 'operator:ana' }))

opCase('safe-cleared-by-nobody', 'An empty clearance is not a clearance. A caller with nobody to '
  + 'name should not have one, and `""` must not be a way to unstick SAFE in one edition and not '
  + 'the other.',
  'safe', opConditions({ cleared_by: '' }))

opCase('safe-cleared-while-the-condition-persists', 'What made it unnecessary to invent a '
  + 'destination for a cleared SAFE, which §27 does not name. Clearing clears the *stickiness*; the '
  + 'conditions still decide. A robot whose estop is still pressed goes straight back — with the '
  + 'reason that put it there rather than the one it was cleared by.',
  'safe', opConditions({ stop: 'operator', stop_settled: true, cleared_by: 'operator:ana' }))

opCase('safe-with-a-failing-self-test-reports-the-self-test', 'And an uncleared SAFE whose '
  + 'condition still holds reports the condition rather than the stickiness: `safe_sticky` would '
  + 'send an operator to press a button they already pressed.',
  'safe', opConditions({ post: 'failed' }))

// Idempotence, which is what makes this safe to call from a level-triggered loop.

opCase('re-evaluating-the-state-it-produced', 'Running from `running` under unchanged conditions '
  + 'changes nothing and sounds nothing. §33.3’s loop runs on a timer and on every event, so a '
  + 'machine that re-announced itself would fill a black box with transitions that did not happen.',
  'running', opConditions())

opCase('re-entering-hold-from-hold', 'The same property on the resting side.',
  'hold', opConditions({ policy: 'none' }))

// The properties, and the matrix.

add('opstate', 'properties', {
  id: 'every-state',
  note: 'What each state means to the three sections that read it: §33.4’s safe-state barrier, '
    + '§49.1’s tone, and §27’s stickiness. `stopping` is the row worth reading — a robot '
    + 'mid-ramp most looks like it is about to be safe and most certainly is not, and an OS update '
    + 'applied across it would change the meaning of the deceleration in flight.',
  input: {},
  expected: canonical(shapeStateProperties()),
})

add('opstate', 'warning', {
  id: 'every-transition',
  note: 'All forty-nine ordered pairs, and §49.1’s rule over them: the tone sounds when a state '
    + 'that commands autonomously is entered from one that does not. Frozen in full rather than '
    + 'sampled, because the failure mode of this predicate is a robot beginning to move in silence '
    + '— and the version this replaced was under-inclusive for exactly one pair family, `→ '
    + 'degraded`, which no sampled corpus would have been guaranteed to hold.',
  input: {},
  expected: canonical(shapeWarningMatrix()),
})

// ── §54.4's lifecycle ────────────────────────────────────────────────────────
//
// The state machine the four pure decisions above were frozen *for*, and which two sessions
// recorded as blocked on capability leases and on §27's machine. Both exist, so this does.
//
// Every case pins the phase, the reason, the outcome and `release_leases` together, because they
// are one answer and an edition could get the phase right while releasing a lease at the wrong
// moment — which is the failure that has no symptom until the next invocation is refused the arm.

function progress(overrides: SkillProgress = {}): SkillProgress {
  return {
    invoked: true,
    root: true,
    entry_permitted: true,
    leases: 'held',
    elapsed_ms: 100,
    deadline: skillDeadline(10, DEFAULT_OVERRUN_FACTOR),
    robot: 'running',
    ...overrides,
  }
}

function lifecycleCase(
  id: string,
  note: string,
  phase: SkillPhase,
  state: SkillProgress,
): void {
  add('skill', 'lifecycle', {
    id,
    note,
    input: { phase, progress: state },
    expected: canonical(shapeSkillTransition(advanceSkill(phase, state))),
  })
}

lifecycleCase('nobody-asked', 'The value of an invocation nobody has made. Every default is "not '
  + 'gathered", and nothing about it advances — the same opening assertion §27\'s machine has, for '
  + 'the same reason.',
  'idle', {})

lifecycleCase('invoked', 'An invocation arrived. §54.1: a skill is invoked with arguments and '
  + 'terminates, which is the whole distinction from a policy.',
  'idle', progress())

lifecycleCase('preconditions-unevaluated',
  'The case that matters most in this file. A runtime that has not evaluated §54.2 yet must not be '
  + 'able to start a skill by failing to ask, so `null` holds the machine where it is rather than '
  + 'letting it through. "Not assume true, which is what free-text preconditions do by '
  + 'construction."',
  'preconditions', progress({ entry_permitted: null }))

lifecycleCase('preconditions-refused',
  '§54.6\'s `skill.preconditions.enforced`: refused, with the predicate named. The naming is shared '
  + 'between two functions — `evaluatePreconditions` finds the clause and the lifecycle carries it '
  + 'into what an operator reads — and neither is complete alone.',
  'preconditions', progress({ entry_permitted: false, refused_clause: 'the cell door is closed' }))

lifecycleCase('preconditions-refused-unnamed',
  'And a refusal with no clause to name still refuses. A caller that lost the report must not be '
  + 'able to turn a refusal into a start.',
  'preconditions', progress({ entry_permitted: false }))

lifecycleCase('preconditions-met', 'On to §25.2.', 'preconditions', progress())

lifecycleCase('nested-call-does-not-acquire',
  '§54.5: "leases are held by the root invocation, not re-acquired per call. Nested acquisition is '
  + 'how you get a skill deadlocking against its own child." The one composition limit that is not '
  + 'a graph property, and it lives here.',
  'preconditions', progress({ root: false }))

lifecycleCase('lease-pending',
  'A lease nobody has taken is not one that was granted — the same rule as the precondition above, '
  + 'arrived at from §25.2 rather than from §54.2.',
  'acquiring', progress({ leases: 'pending' }))

lifecycleCase('lease-denied',
  '§25.2 refuses rather than interleaves: "two policies alternating commands to the same joint '
  + 'produce a trajectory neither was trained on and neither can be held responsible for." The '
  + 'token and the holder reach the detail, because an operator has to know which package to stop.',
  'acquiring', progress({
    leases: 'denied',
    lease_token: 'manipulation.grasp',
    lease_holder: 'acme/pick-place',
  }))

lifecycleCase('power-withheld',
  '§36.1\'s `low` row, which was a sentence in the specification and a rule nothing obeyed until '
  + '§36 was written. No new task leases are granted, so the invocation aborts rather than waiting: '
  + 'an invocation parked in `acquiring` hoping for a charger burns §54.4\'s timeout and then '
  + 'aborts with the wrong reason on it.',
  'acquiring', progress({
    power_withheld: "§36.1 withholds new task leases: this robot's power state is `reserve` "
      + '(at_reserve)',
  }))

lifecycleCase('power-withheld-outranks-a-held-token',
  'Both refusals true at once. Power answers first, and the order is not a preference: a refusal '
  + 'for want of energy is a fact about the robot and a held token is a fact about another package, '
  + 'so the robot-wide one is what an operator needs. It is also the only order that works — a '
  + 'runtime that withholds does not ask §25.2\'s table, so `leases` would read `pending`.',
  'acquiring', progress({
    power_withheld: '§36.1 withholds new task leases: this robot\'s power state is `critical`',
    leases: 'denied',
    lease_token: 'manipulation.grasp',
    lease_holder: 'acme/pick-place',
  }))

lifecycleCase('power-withheld-empty-is-not-a-refusal',
  'An empty sentence is not a refusal. §64 makes a refusal name its fix, and a skill aborted with a '
  + 'blank reason on it is the one shape that cannot — so a caller that sets the field to nothing '
  + 'gets the acquiring rules exactly as they were.',
  'acquiring', progress({ power_withheld: '   ', leases: 'held' }))

lifecycleCase('lease-never-granted',
  'The other half of §25.2, and the one no case had. A token the operator never granted reports no '
  + 'holder — "nobody has the arm, this package may not ask for it" — and the refusal has to say '
  + 'that rather than "held by another package", which sends an operator hunting for a package that '
  + 'does not exist when the fix is one grant. Found by writing a runtime, not by reading the '
  + 'document.',
  'acquiring', progress({
    leases: 'denied',
    lease_token: 'manipulation.grasp',
    lease_holder: '',
  }))

lifecycleCase('lease-denied-with-nothing-named',
  'And a denial that names no token at all says so, rather than blaming a package. This is the '
  + 'shape a caller produces by forgetting a field, and inventing a holder for it would be the same '
  + 'lie in a quieter place.',
  'acquiring', progress({ leases: 'denied' }))

lifecycleCase('leases-acquired', 'Running.', 'acquiring', progress())

lifecycleCase('running', 'Still running is a fixed point, which is what makes this safe to call '
  + 'every cycle from a loop that runs faster than the work.',
  'running', progress())

lifecycleCase('timeout',
  '§54.6\'s `skill.timeout.enforced`. A 10 s estimate at the default factor of 2 is 20 000 ms, and '
  + 'the comparison is inclusive: a skill that has used exactly its budget has used it.',
  'running', progress({ elapsed_ms: 20_000 }))

lifecycleCase('just-inside-the-deadline', 'One millisecond the other side of it.',
  'running', progress({ elapsed_ms: 19_999 }))

lifecycleCase('unenforceable-deadline-never-expires',
  'A skill that declares no duration produces an unenforceable deadline, and `skillDeadline` '
  + 'deliberately does not decide whether such a skill may run. Neither does this: what it does not '
  + 'do is invent a ceiling.',
  'running', progress({ deadline: skillDeadline(undefined, DEFAULT_OVERRUN_FACTOR), elapsed_ms: 1e12 }))

lifecycleCase('hold-lost',
  '§54.6\'s `skill.preconditions.rechecked`. §54.2: "battery dropping below reserve mid-skill '
  + 'should stop the skill, not be discovered at the end."',
  'running', progress({ hold_lost: true }))

lifecycleCase('guard-tripped', 'A skill is not an exemption from the kernel.',
  'running', progress({ guard_tripped: true }))

for (const state of ['stopping', 'safe', 'post', 'unhomed'] as const) {
  lifecycleCase(`robot-${state}`,
    `§54.4's \`running\` row read through §27's machine: \`${state}\` is not a state a policy `
    + 'commands in, so a skill running on a robot in it is over.',
    'running', progress({ robot: state }))
}

for (const state of ['hold', 'degraded'] as const) {
  lifecycleCase(`robot-${state}-does-not-abort`,
    state === 'hold'
      ? 'The exclusion worth pinning. `hold` is where §27\'s machine sits between a lease being '
        + 'granted and a policy actually commanding, so aborting on it would kill every skill in '
        + 'the moment it started. A guard asking for a hold arrives as `guard_tripped` instead.'
      : '§27\'s `DEGRADED` row runs. §45.5 makes an unavailable guard the ordinary case, and a '
        + 'skill refusing to proceed on one would refuse on most real robots.',
    'running', progress({ robot: state }))
}

lifecycleCase('the-cause-outranks-its-consequence',
  'Every abort condition at once. A timeout makes the OS stop the skill under a controlled stop and '
  + 'a guard trip stops the robot, so both *produce* a robot that is no longer commanding — and '
  + 'reporting the robot state here would hide the cause behind its own consequence. Timeout wins.',
  'running', progress({
    elapsed_ms: 20_000, hold_lost: true, guard_tripped: true, robot: 'stopping',
  }))

lifecycleCase('finished', 'Verified rather than believed. §54.3: "a skill that judges its own '
  + 'success is a skill that always succeeds."',
  'running', progress({ finished: true }))

lifecycleCase('verdict-pending', 'And an unjudged skill is not a successful one.',
  'verifying', progress({ finished: true }))

for (const outcome of SKILL_OUTCOMES) {
  lifecycleCase(`judged-${outcome}`,
    `§54.3's verdict reaches the terminal phase unchanged. \`${outcome}\` is the Hub's `
    + '`EpisodeOutcome` and not a second vocabulary, so an aborted skill becomes a labelled '
    + 'training example rather than a gap.',
    'verifying', progress({ finished: true, outcome }))
}

lifecycleCase('abort-during-verification',
  'A skill whose robot fell over during verification did not succeed at anything, whatever a '
  + 'criterion evaluated to on the way down. The abort outranks the verdict.',
  'verifying', progress({ finished: true, outcome: 'success', guard_tripped: true }))

lifecycleCase('already-over',
  'A skill already terminal releases nothing. A release emitted every cycle after a skill ended '
  + 'would be a release racing the next invocation\'s acquisition, which is the one way §25.2\'s '
  + 'arbitration can be lost by the thing that implements it.',
  'terminal', progress({ outcome: 'success' }))

// ── §54.5's third limit ──────────────────────────────────────────────────────

function boundedCase(
  id: string,
  note: string,
  expected_duration_seconds: number | undefined,
  rootRemainingMs: number | null,
): void {
  const child = skillDeadline(expected_duration_seconds, DEFAULT_OVERRUN_FACTOR)
  add('skill', 'bounded-deadline', {
    id,
    note,
    input: { expected_duration_seconds: expected_duration_seconds ?? null, root_remaining_ms: rootRemainingMs },
    expected: canonical(shapeDeadline(boundedDeadline(child, rootRemainingMs))),
  })
}

boundedCase('root-bounds-a-longer-child',
  '§54.5: "the root\'s timeout bounds the whole tree. A child cannot extend its parent\'s budget."',
  60, 5_000)

boundedCase('a-shorter-child-keeps-its-own',
  'The bound is a minimum, not a replacement.', 1, 90_000)

boundedCase('an-unbounded-child-is-still-bounded',
  'A child that declares no duration is bounded by its parent, so an unbounded skill nested under a '
  + 'bounded one does not make the tree unbounded — which is the composition failure §54.5 names.',
  undefined, 7_500)

boundedCase('a-root-with-nothing-left-bounds-at-zero',
  'Zero rather than a negative number: a deadline already in the past is one some other arithmetic '
  + 'could read as "no deadline", and the child would outlive the tree.',
  60, -1)

boundedCase('an-unbounded-root-leaves-the-child-alone',
  'Including leaving it unbounded. Nothing here invents a ceiling for a tree nobody bounded.',
  60, null)

boundedCase('neither-is-bounded',
  'And the honest answer for two skills that declared nothing is still nothing.', undefined, null)

// ── §54.2's live context ─────────────────────────────────────────────────────
//
// The half of §54.2 that had no builder until a runtime needed one. `evaluatePreconditions` has
// always taken a context as an argument, and nothing said what a robot puts in it — so every case
// here is really a case about one rule: **a fact the robot does not know is absent, not defaulted.**
//
// The failure this pins is invisible from either side alone. An edition that filled an unknown
// battery with `0` and an edition that filled it with `100` both look reasonable in a diff; one
// refuses every skill and the other runs every skill, and no test of `evaluatePreconditions` would
// catch either, because both would be handed a context that resolves.

function liveCase(id: string, note: string, inputs: LiveInputs): void {
  add('live', 'context', {
    id,
    note,
    input: inputs as unknown as Record<string, unknown>,
    expected: canonical(shapeLiveContext(liveContext(inputs))),
  })
}

liveCase('nothing-observed',
  'A robot nobody has observed. Six of the seven branches are absent; the one that is present is '
  + 'an empty array, because a grant is a file the daemon owns and "this package was granted '
  + 'nothing" is an answer rather than an absence. Everything else omits, because §54.2 refuses a '
  + 'clause naming an unresolvable path and that refusal is the honest answer for a fact nobody '
  + 'measured.',
  {})

liveCase('a-kernel-that-reports-no-guard-running',
  'The distinction the guards branch exists to carry, and the correction this module made to '
  + 'itself. An empty `robot.guards.active` is a kernel saying no guard is evaluating; an absent '
  + 'one is nobody having asked a kernel. §33.1 puts `skynetd` on the far side of that line, so '
  + 'the empty list it would otherwise supply would be a claim about a guard set — which is exactly '
  + 'what §54.2’s first rule refuses to let anybody make.',
  { guards_active: [] })

liveCase('a-fully-observed-robot',
  'Every branch §54.2 names, populated. The shape a skill publisher writes predicates against.',
  {
    joints: [
      { name: 'shoulder_pan', position: 0.25 },
      { name: 'elbow', position: -1.5 },
    ],
    gripper: 0.8,
    battery_percent: 62,
    base_pose: { xyz: [1, 2, 0], quaternion: [0, 0, 0, 1] },
    operating_state: 'running',
    guards_active: ['envelope', 'rate_limit', 'watchdog'],
    guards_unavailable: ['workspace'],
    calibration: [
      { set: 'joint_zeros', validity: { state: 'valid', checked: ['age'] }, age_s: 3_600 },
      { set: 'camera_extrinsics', validity: { state: 'absent' } },
    ],
    frames_resolved: ['base_link', 'tool0'],
    tier: 'host',
    grants: ['arm.left', 'sensing.camera'],
    env: { cell: 'bench', door_closed: true, fixtures: ['jig'] },
  })

liveCase('a-battery-nobody-can-read',
  'The case the whole module exists for. A robot with no battery sensor omits '
  + 'robot.state.battery_percent, so a skill that asked about it is refused *with the path named*. '
  + 'Filling it with 0 would refuse every skill and filling it with 100 would run every skill, and '
  + 'both are one-line changes that look reasonable in a diff.',
  { joints: [{ name: 'elbow', position: 0 }], operating_state: 'running' })

liveCase('a-joint-reporting-nan',
  'The fault drivers/mock injects. A non-finite reading is not a reading: the joint is absent from '
  + 'the context and named in `unreadable`, so the precondition about it is refused and an operator '
  + 'is told why rather than finding a joint quietly missing.',
  { joints: [{ name: 'elbow', position: Number.NaN }, { name: 'wrist', position: 0.1 }] })

liveCase('every-joint-unreadable',
  'And robot.state.joints is then absent rather than an empty object. An empty object resolves, so '
  + '`exists` on a joint path would answer *false* about a robot whose sensors are all failing — '
  + 'which is the same word it uses for a body that has no such joint.',
  { joints: [{ name: 'elbow', position: Number.NaN }] })

liveCase('a-joint-whose-name-carries-a-dot',
  'cell.ts found this from the other side and could refuse the key, because an operator can rename '
  + 'a fact. Nobody can rename a body’s joint, so the name is reported instead: no precondition '
  + 'will ever be able to address robot.state.joints.arm.1, and an operator who can see the joint '
  + 'on the console needs to be told that rather than left to infer it from a refusal.',
  { joints: [{ name: 'arm.1', position: 0.5 }, { name: 'arm_2', position: 0.5 }] })

liveCase('grants-are-a-list-because-tokens-carry-dots',
  'The decision this file makes that §54.2 does not. `skill.grants.<token>` is unaddressable for '
  + 'every token that exists — arm.left, sensing.camera and locomotion.base all carry a dot — so '
  + 'the branch is a sorted list read with `includes`, and the shape changes rather than the names.',
  { grants: ['sensing.camera', 'arm.left', 'arm.left'] })

liveCase('a-base-pose-with-one-bad-number',
  'All seven numbers or none. §54.2 names "base pose" as one fact, and a context that published a '
  + 'position with no orientation would let a precondition about a docked robot read true while it '
  + 'faced the wrong way.',
  { base_pose: { xyz: [1, Number.NaN, 0], quaternion: [0, 0, 0, 1] } })

liveCase('a-calibration-set-with-no-clock',
  'Valid, and of unknown age. §54.2 asks for "which parameter sets are valid, and their age", and a '
  + 'robot with no wall clock knows the first and not the second — so `valid` is published and '
  + '`age_s` is absent. Reporting an age of zero would make a stale calibration read as one taken '
  + 'this second.',
  { calibration: [{ set: 'joint_zeros', validity: { state: 'valid', checked: [] } }] })

liveCase('every-calibration-state',
  'All five, so an edition that collapsed `failed` into `absent` — or `unusable` into `failed`, '
  + 'which §45.3 spends a paragraph separating — diverges here rather than in a support call. Only '
  + '`valid` sets the boolean; the state word carries the rest.',
  {
    calibration: [
      { set: 'joint_zeros', validity: { state: 'valid', checked: ['age'] }, age_s: 10 },
      { set: 'camera_extrinsics', validity: { state: 'absent' } },
      { set: 'hand_eye', validity: { state: 'failed', detail: 'residual 4 exceeds 1' } },
      { set: 'imu_bias', validity: { state: 'expired', reason: 'age', detail: 'older than 86400s' } },
      { set: 'force_torque_offset', validity: { state: 'unusable', detail: 'no method recorded' } },
    ],
  })

liveCase('a-frame-graph-that-does-not-flatten',
  '§44’s fault reaches a precondition whole — code, subjects and sentence — beside an empty '
  + 'resolved list, so a skill that needs a frame is refused by `includes_all` over nothing rather '
  + 'than by a path that does not resolve. Both refuse; only one tells the operator that the graph '
  + 'is broken rather than that the frame is missing. The fault is the validator’s own shape and '
  + 'not a string, which is the §64 invariant over the corpus arriving as a design constraint: '
  + '`fault` in this system means something an operator can act on.',
  {
    frames_resolved: [],
    frames_fault: {
      code: 'parent_missing',
      subjects: ['tool0'],
      detail: 'tool0 names a parent nothing declares.',
    },
  })

liveCase('an-unobserved-operating-state',
  'Absent rather than `post`. §27’s machine defaults to POST because that is where a booting robot '
  + 'is; a *context* that defaulted to it would be stating a state nobody measured, and a '
  + 'precondition reading it would get an answer rather than a refusal.',
  { battery_percent: 90 })

liveCase('duplicated-and-unsorted-lists',
  'Every list is sorted and de-duplicated on the way in, because two editions that agreed about the '
  + 'contents and disagreed about the order would produce different bytes for the same robot — and '
  + '`includes` would not notice, so nothing but the corpus would catch it.',
  {
    guards_active: ['workspace', 'envelope', 'workspace'],
    frames_resolved: ['tool0', 'base_link', 'tool0'],
    grants: ['b.token', 'a.token', 'b.token'],
  })

liveCase('an-env-branch-with-no-cell',
  'A robot with no cell profile keeps the other six branches. §55.2 makes the cell a site-local '
  + 'document that may simply not exist yet, and a context that refused to build without one would '
  + 'take the battery check away from every robot nobody has surveyed.',
  { battery_percent: 55, operating_state: 'hold' })

// ── §12's two decisions ──────────────────────────────────────────────────────
//
// The third rule is the one §12 calls the single most important test in the suite:
//
// > An update that changes the Safety Kernel or the envelope is refused without a fresh bench pass.
// > Without this rule, every guarantee in §7 and §15 is one silent update away from being void.
//
// Every case below that touches it is really asking one question: can this gate be got past by
// somebody who wants to. The answers frozen here are that it cannot be got past by renaming the
// component, by holding a pass for other bytes, by holding a pass that failed, or by shipping a
// bundle with no digest for a pass to be bound to.

function bundleCase(
  id: string,
  note: string,
  bundle: BundleManifest,
  subject: UpdateSubject,
): void {
  add('ota', 'bundle', {
    id,
    note,
    input: { bundle, subject },
    expected: canonical(shapeBundleVerdict(admitBundle(bundle, subject))),
  })
}

const SIGNED_BUNDLE: BundleManifest = {
  version: '1.4.0',
  channel: 'stable',
  digest: 'sha256:aaaa',
  compatible: ['bench-arm-v1'],
  changes: ['userspace'],
}

const HEALTHY_SUBJECT: UpdateSubject = {
  running_version: '1.3.0',
  board: 'bench-arm-v1',
  channel: 'stable',
  signature: 'verified',
}

bundleCase('an-ordinary-update',
  'A signed image for this board, on the channel the desired state asked for, changing nothing '
  + '§12 gates. Admitted, with no findings at all — the case that has to hold or every other one '
  + 'is measuring a function that refuses everything.',
  SIGNED_BUNDLE, HEALTHY_SUBJECT)

bundleCase('a-kernel-change-with-no-bench-pass',
  '§12’s third rule, stated plainly. This is the refusal the whole section exists for: an update '
  + 'touching the safety kernel, on a robot that holds no bench pass, is refused — and the '
  + 'component is named so an operator knows what they are booking bench time for.',
  { ...SIGNED_BUNDLE, changes: ['safety_kernel', 'userspace'] }, HEALTHY_SUBJECT)

bundleCase('an-envelope-change-with-no-bench-pass',
  'The second of §12’s two names, and it is not a code change at all — an envelope is data in a '
  + 'descriptor. That is exactly why the document names it beside the kernel: the guarantees in §7 '
  + 'rest on the numbers as much as on the code that enforces them.',
  { ...SIGNED_BUNDLE, changes: ['envelope'] }, HEALTHY_SUBJECT)

bundleCase('a-kernel-change-a-bench-pass-covers',
  'And the gate opens for evidence bound to these exact bytes. Admitted.',
  { ...SIGNED_BUNDLE, changes: ['safety_kernel'] },
  { ...HEALTHY_SUBJECT, bench: { image_digest: 'sha256:aaaa', tier: 'host', passed: true } })

bundleCase('a-bench-pass-for-other-bytes',
  'The anti-gaming case. A robot that has passed a bench test *at some point* is every robot the '
  + 'gate exists to protect, so the pass is bound to a digest — and one taken against 1.3.0 says '
  + 'nothing about 1.4.0. The same rule §56.3 applies to benchmark seeds, arriving in a different '
  + 'section.',
  { ...SIGNED_BUNDLE, changes: ['safety_kernel'] },
  { ...HEALTHY_SUBJECT, bench: { image_digest: 'sha256:bbbb', tier: 'host', passed: true } })

bundleCase('a-bench-pass-that-failed',
  'A pass for the right bytes that records a failure. §12 refuses an update that has not passed, '
  + 'which is a different sentence from one that has not been tried — and a gate reading only '
  + '"is there a record" would admit this one.',
  { ...SIGNED_BUNDLE, changes: ['safety_kernel'] },
  { ...HEALTHY_SUBJECT, bench: { image_digest: 'sha256:aaaa', tier: 'host', passed: false } })

bundleCase('a-gated-change-renamed',
  'The way somebody would actually get past this gate: not by writing `safety_kernel`, but by '
  + 'writing `rt-core`. An unrecognised component is gated for the same reason the two named ones '
  + 'are — nobody has established what it changes — and the refusal says so rather than reporting '
  + 'an unknown word as harmless.',
  { ...SIGNED_BUNDLE, changes: ['rt-core'] }, HEALTHY_SUBJECT)

bundleCase('a-gated-change-with-nothing-to-bind-a-pass-to',
  'A bundle with no digest cannot be the subject of evidence, so a kernel change in one is refused '
  + 'twice over and the two refusals are separate findings. An operator who fixed only the missing '
  + 'digest would still not be able to apply it, and being told both at once is the point.',
  { ...SIGNED_BUNDLE, digest: '', changes: ['safety_kernel'] },
  { ...HEALTHY_SUBJECT, bench: { image_digest: '', tier: 'host', passed: true } })

for (const signature of SIGNATURE_STATES) {
  if (signature === 'verified') continue
  bundleCase(`signature-${signature}`,
    `§37.1’s states are four different facts with four different people at the end of them, and an `
    + `OS image is where that matters most: \`failed\` is what tampering looks like, \`untrusted\` `
    + `is a decision nobody has made, \`absent\` is the publisher’s problem and \`unverifiable\` is `
    + `the operating system’s.`,
    SIGNED_BUNDLE, { ...HEALTHY_SUBJECT, signature })
}

bundleCase('an-image-for-another-board',
  'Refused, and the refusal names both what the bundle is for and what this machine is — because '
  + 'the operator reading it is deciding whether the bundle is wrong or the inventory is.',
  { ...SIGNED_BUNDLE, compatible: ['warehouse-amr-v2'] }, HEALTHY_SUBJECT)

bundleCase('a-machine-that-does-not-know-what-it-is',
  'A warning rather than a refusal. A robot that cannot report its board has not established a '
  + 'mismatch, and refusing would make an unknown machine unupdatable — which is the state a robot '
  + 'is in exactly when it most needs an update.',
  SIGNED_BUNDLE, { ...HEALTHY_SUBJECT, board: null })

bundleCase('a-bundle-that-declares-no-board',
  'Also a warning, and for a different reason: §12 does not require the field, so refusing would '
  + 'stop every image built before it existed. It is said out loud because "this image does not '
  + 'know what it is for" is what somebody wants to have been told after flashing the wrong board.',
  { ...SIGNED_BUNDLE, compatible: [] }, HEALTHY_SUBJECT)

bundleCase('a-bundle-from-another-channel',
  'Refused. A robot pulled onto `beta` by an image rather than by an operator is a robot nobody '
  + 'moved.',
  { ...SIGNED_BUNDLE, channel: 'beta' }, HEALTHY_SUBJECT)

bundleCase('the-version-already-running',
  'A warning, not a refusal: staging it again is harmless and applying it costs a reboot for '
  + 'nothing. Refusing would break a robot re-converging on a desired state it already satisfies, '
  + 'which is the ordinary case after a rollback.',
  SIGNED_BUNDLE, { ...HEALTHY_SUBJECT, running_version: '1.4.0' })

bundleCase('a-bundle-that-declares-nothing',
  'Everything wrong at once, so the ordering of the findings is frozen. An operator gets one list '
  + 'rather than four reboots.',
  {}, { signature: 'absent' })

// ── §12's second rule ────────────────────────────────────────────────────────

function bootCase(id: string, note: string, evidence: BootEvidence): void {
  add('ota', 'boot', {
    id,
    note,
    input: evidence as unknown as Record<string, unknown>,
    expected: canonical(shapeBootVerdict(confirmBoot(evidence))),
  })
}

bootCase('an-ordinary-boot',
  'Not the first boot into a new slot, so there is nothing to confirm and nothing to roll back. '
  + 'The case that keeps a supervisor from marking a slot bad on a robot nobody updated.',
  {})

bootCase('nobody-has-checked-anything-yet',
  'The case that matters most in this file. A supervisor that marked a slot good because it had '
  + 'not yet been told otherwise would replace §12’s unattended recovery with a race between the '
  + 'health check and the confirmation, so absence of evidence waits.',
  { pending: true, elapsed_ms: 1_000 })

bootCase('healthy',
  'The self-test passed, every wanted package is running and the reconciler converged. Mark it '
  + 'good, and the bootloader stops counting.',
  {
    pending: true,
    elapsed_ms: 42_000,
    post_passed: true,
    packages_healthy: true,
    converged: true,
  })

bootCase('two-of-three',
  'Still waiting, and the report names which one is missing — because "the update did not take" '
  + 'and "one driver would not start" send an engineer to different places.',
  { pending: true, elapsed_ms: 60_000, post_passed: true, packages_healthy: true })

bootCase('the-budget-expires',
  '§12’s unattended recovery, which is the whole reason A/B costs the storage it costs. Ten '
  + 'minutes in, the self-test has not passed; mark the slot bad and let the bootloader return.',
  { pending: true, elapsed_ms: 600_000, post_passed: false })

bootCase('one-millisecond-inside-the-budget',
  'The boundary, frozen, because a comparison that was `>` rather than `>=` is one character and '
  + 'would leave a robot waiting for ever at exactly the moment the budget ran out.',
  { pending: true, elapsed_ms: 599_999, post_passed: false })

bootCase('exactly-at-the-budget',
  'And the other side of it.',
  { pending: true, elapsed_ms: 600_000, post_passed: false })

bootCase('a-budget-of-zero',
  'Waits rather than rolling back. A budget that is not a positive number is no budget, and the '
  + 'patient reading is the safe one here: an unattended rollback triggered by a misconfigured '
  + 'budget is a working robot rebooted into an older image for no reason.',
  { pending: true, elapsed_ms: 5_000, budget_ms: 0, post_passed: false })

bootCase('a-narrowed-budget',
  'An operator may narrow the ten-minute default, and the sentence carries the number they set '
  + 'rather than the one this file ships.',
  { pending: true, elapsed_ms: 31_000, budget_ms: 30_000, post_passed: true, packages_healthy: true })

bootCase('healthy-but-not-pending',
  'Health on an ordinary boot decides nothing. Confirming a slot nobody staged would be a '
  + 'supervisor telling the bootloader about an update that never happened.',
  { post_passed: true, packages_healthy: true, converged: true })

vocabulary('os_components',
  'What an OS bundle may declare that it changes — the stack §3 draws, and nothing invented beyond '
  + 'it. Two entries carry a rule; the rest are here so that an unrecognised name is recognisable '
  + 'as unrecognised.',
  OS_COMPONENTS)
vocabulary('certification_gated',
  '§12’s two names. Not the whole rule — an unrecognised component is gated too — which is why '
  + 'this set is frozen beside `os_components` rather than instead of it.',
  CERTIFICATION_GATED)
vocabulary('bundle_faults', 'Everything §12 can find wrong with an image, in the order the checks '
  + 'run.', BUNDLE_FAULTS)
vocabulary('boot_decisions',
  'What a supervisor may tell a bootloader, weakest action first. `wait` is the default and the '
  + 'only one of the four that is safe to reach by accident.',
  BOOT_DECISIONS)

// ── §10's three decisions ────────────────────────────────────────────────────
//
// The pairing cases are the ones worth reading twice. Every one of them is really asking the same
// question: does a robot that has just been handed eight characters by a person do something
// useful, and does it do it **without ever repeating them back**. A code is a bearer secret for
// the length of a pairing, and the refusals frozen here are refusals that name a position and a
// length and never a character — which is a property a reviewer can check by reading the
// `expected` strings, and which is exactly what an edition would lose by writing the obvious
// error message.

function codeCase(id: string, note: string, input: string): void {
  add('identity', 'pairing', {
    id,
    note,
    input: { input },
    expected: canonical(shapeCodeResult(normalizePairingCode(input))),
  })
}

codeCase('the-specifications-own-example',
  '§18 writes `skynet pair ABCD-1234`. If this case ever stops producing a code, either the '
  + 'alphabet or the length has drifted away from the document’s own example — which is the one '
  + 'string in the specification a user will actually type.',
  'ABCD-1234')

codeCase('the-same-code-without-its-hyphen',
  'The hyphen is how a code is printed, not part of it. Two spellings of one code must not be two '
  + 'codes.',
  'ABCD1234')

codeCase('lower-case',
  'Nobody holds shift for eight characters, and a code that only worked in upper case would be a '
  + 'support call per robot.',
  'abcd-1234')

codeCase('read-off-a-screen-and-typed-badly',
  'Leading and trailing whitespace from a paste, an inner space where the hyphen was, and a '
  + 'trailing newline. All cosmetic.',
  '  abcd 1234\n')

codeCase('hyphens-everywhere',
  'Somebody grouping the characters their own way still gets their code. The rule is that hyphens '
  + 'and spaces do not count, not that they appear in one place.',
  'A-B-C-D-1-2-3-4')

codeCase('the-letters-that-fold',
  'Crockford’s folding, and the reason the alphabet excludes these letters rather than rejecting '
  + 'them: an operator reading `01234567` off a screen types the letters they think they see, and '
  + 'refusing that would be refusing a correct code because a font has a slashed zero.',
  'OI234567')

codeCase('a-lower-case-l-folds-too',
  'The worst of the three, because a lower-case l and a 1 are the same glyph in most sans-serif '
  + 'faces. Folded after upper-casing, so `l` and `L` reach the same digit.',
  'llll2345')

codeCase('a-credential-pasted-in',
  'The check that exists because an operator holds two secrets at once and will eventually paste '
  + 'the wrong one. Caught before anything is sent, and the refusal names the class rather than '
  + 'the value — a credential echoed into a terminal is a credential in a shell history.',
  'skd_abcdef0123456789')

codeCase('a-credential-in-upper-case',
  'Somebody’s terminal, or somebody retyping. The prefix match is case-insensitive for the same '
  + 'reason the code is.',
  'SKD_ABCDEF0123456789')

codeCase('a-credential-too-short-to-be-a-code',
  'Five characters, so a parser that stripped first would report it too short and send the '
  + 'operator hunting for three missing characters in a string that is not a code at all. The '
  + 'order of the checks is what this case pins.',
  'skd_x')

codeCase('nothing-at-all',
  'The empty string reaches this function whenever somebody runs the command with no argument, and '
  + 'the sentence has to say what a code looks like rather than that a string was empty.',
  '')

codeCase('only-hyphens',
  'Not empty, and nothing left after the cosmetics are removed. Reported as empty, because that is '
  + 'what the operator has.',
  '----')

codeCase('seven-characters',
  'One short. Named as a length rather than as a missing character, because nobody can say which '
  + 'one is missing.',
  'ABCD123')

codeCase('nine-characters',
  'One long, and the other side of the same boundary.',
  'ABCD12345')

codeCase('a-u-in-the-middle',
  'The letter Crockford drops. It cannot have been read off a screen, so it is a mistyped '
  + 'something-else — and the refusal says which position rather than which character.',
  'ABCDUFGH')

codeCase('a-character-nobody-could-have-read',
  'Punctuation, from a paste that took a neighbouring character.',
  'ABCDEF@1')

codeCase('the-first-character-is-the-bad-one',
  'The boundary of the position count. 1-based, because it is read by a person counting '
  + 'characters on a screen and not by a program indexing an array.',
  '!BCD1234')

codeCase('eight-characters-that-are-not-ascii',
  'Eight code points, so it is not a length fault, and the first of them is not in the alphabet. '
  + 'Counted in characters rather than bytes: telling somebody they typed twenty-four characters '
  + 'would be a sentence about an encoding they did not choose.',
  'αβγδεζηθ')

// ── §19's device credential, and §10's key binding ───────────────────────────

function identityCase(id: string, note: string, identity: Identity, observed: Observed): void {
  add('identity', 'credential', {
    id,
    note,
    input: { identity, observed },
    expected: canonical(shapeIdentityVerdict(checkIdentity(identity, observed))),
  })
}

const PAIRED: Identity = {
  device_id: 'dev_7',
  namespace: 'acme',
  device_key: 'aa',
  binding: 'hardware',
  credential: true,
  credential_prefix: 'skd_',
  scopes: ['device:checkin', 'device:desired'],
}

const ATTESTED: Observed = { birth_key: 'aa', birth_certificate: true }

identityCase('a-robot-in-its-box',
  '§50.2: it ships with a certificate and no namespace, and belongs to nobody yet. Every finding '
  + 'here is a warning and none of them is a failure — a robot at the correct point in its '
  + 'lifecycle must not read as broken, or the first thing anybody learns about this surface is '
  + 'to ignore it.',
  {}, {})

identityCase('paired-and-attested',
  'The whole of §10 satisfied: a device credential, a namespace, a key inside a secure element, '
  + 'and a vendor who signed that exact key. Nothing found. The case that has to hold or every '
  + 'other one is measuring a function that complains about everything.',
  PAIRED, ATTESTED)

identityCase('paired-on-a-key-in-a-file',
  'Guest tier’s ordinary state, and the ceiling §10 puts on `security.identity.attested`. A '
  + 'warning, because refusing it would make the tier unreachable on every machine without a TPM '
  + '— which is most of them, and all of the ones anybody develops on.',
  { ...PAIRED, binding: 'software' }, ATTESTED)

identityCase('nobody-measured-the-binding',
  'Absent is not `software`. A robot nobody asked has not established it lacks a secure element, '
  + 'and `security.identity.attested` is a measurement — so it is reported unmeasured rather than '
  + 'counted as either answer.',
  { ...PAIRED, binding: null }, ATTESTED)

identityCase('a-credential-with-no-namespace',
  '§10 pairs a robot **to a namespace**. Half a pairing is an error rather than a warning: every '
  + 'report this robot sends afterwards is addressed to a fleet nobody can name.',
  { ...PAIRED, namespace: '' }, ATTESTED)

identityCase('a-credential-of-the-wrong-class',
  'A user token on a robot. §10 puts the narrowest credential in the system in the field on '
  + 'purpose, because that is the one everybody assumes is lost — and a wider one here is a wider '
  + 'one lost.',
  { ...PAIRED, credential_prefix: 'skp_' }, ATTESTED)

identityCase('a-credential-with-no-prefix-at-all',
  'The sentence has to work when there is nothing to quote back, which is the branch a naive '
  + 'format string renders as a pair of empty backticks.',
  { ...PAIRED, credential_prefix: '' }, ATTESTED)

identityCase('an-opaque-bearer-token',
  'The ordinary shape of a token: the scopes live on the Hub and the robot cannot see them. A '
  + 'warning that says the check did not run here — which is a different sentence from saying it '
  + 'passed, and the difference is the whole finding.',
  { ...PAIRED, scopes: null }, ATTESTED)

identityCase('every-scope-a-device-may-carry',
  'The other direction of the allow-list, and it matters as much as the refusal: a list that '
  + 'rejected its own members would make every correctly issued credential an error, which is the '
  + 'shape of check somebody switches off.',
  { ...PAIRED, scopes: [...DEVICE_SCOPES] }, ATTESTED)

identityCase('a-scope-nobody-recognises',
  '§19’s `security.credential.scope`, from the direction that survives a rename. A deny-list of '
  + '§19’s three phrases would wave through a credential that spelled repository read any other '
  + 'way; an allow-list refuses the spelling nobody anticipated, which is the only kind that '
  + 'matters.',
  { ...PAIRED, scopes: ['device:checkin', 'repositories:read'] }, ATTESTED)

identityCase('unrecognised-scopes-are-sorted-and-said-once',
  'Two of them, one repeated. The subjects list is what a console groups by, so a duplicate would '
  + 'render as two problems and an unsorted list would render differently on two editions.',
  {
    ...PAIRED,
    scopes: ['jobs:claim', 'device:checkin', 'repositories:read', 'jobs:claim'],
  },
  ATTESTED)

identityCase('a-mainboard-was-replaced',
  'The finding that only exists because §10 and §50 are both built: the key this robot was paired '
  + 'on is not the key its vendor signed. §50.4 says this is the expected result of a board swap '
  + 'and not a bug — so the message names the remedy, and the severity is still an error, because '
  + 'the alternative reading is a robot paired on a key nobody attested.',
  { ...PAIRED, device_key: 'cc' }, ATTESTED)

identityCase('no-certificate-to-check-the-key-against',
  '§50.3, stated as a finding: without one, what this machine is remains a claim it makes about '
  + 'itself. A warning — a robot with no certificate is not a robot that failed a check, it is a '
  + 'robot where the check could not be run.',
  PAIRED, {})

identityCase('a-certificate-that-names-no-key',
  'A certificate that verified and carries no device key. Nothing to compare, so nothing is '
  + 'claimed — and `attested` is false, because it takes a match and not the absence of a '
  + 'mismatch.',
  PAIRED, { birth_certificate: true })

identityCase('everything-wrong-at-once',
  'The ordering of the findings, frozen. An operator who drove to a warehouse gets one list rather '
  + 'than one problem per attempt.',
  {
    credential: true,
    credential_prefix: 'pat_',
    namespace: '',
    device_key: 'cc',
    binding: 'software',
    scopes: ['repositories:write'],
  },
  ATTESTED)

// ── §10's image attestation ──────────────────────────────────────────────────

function attestCase(id: string, note: string, attestation: Attestation): void {
  add('identity', 'attestation', {
    id,
    note,
    input: attestation as unknown as Record<string, unknown>,
    expected: canonical(shapeAttestationReport(attest(attestation))),
  })
}

const known = (digest: string): Measure => ({ state: 'known', digest })
const nothing = (): Measure => ({ state: 'nothing' })
const unmeasured = (why?: string): Measure => ({ state: 'unmeasured', why: why ?? null })

attestCase('the-sentence-nobody-can-currently-produce',
  '§10, quoted: the Hub records not "robot X is online" but "robot X is running image sha256:…, '
  + 'policy sha256:…, against body sha256:…". This is that sentence, and it is frozen as bytes '
  + 'because it is the one an incident investigation reads.',
  {
    device_id: 'dev_7',
    os_image: known('sha256:aaaa'),
    policy: known('sha256:bbbb'),
    embodiment: known('sha256:cccc'),
  })

attestCase('a-robot-between-policies',
  'Running nothing is a complete and true report, not a missing field. A fleet console that showed '
  + 'this identically to a robot whose policy digest could not be read would be reporting unknown '
  + 'software as none.',
  {
    device_id: 'dev_7',
    os_image: known('sha256:aaaa'),
    policy: nothing(),
    embodiment: known('sha256:cccc'),
  })

attestCase('a-policy-whose-digest-could-not-be-read',
  'The other half of the pair above. Same robot, same shape, and the two must not render the '
  + 'same — which is the entire reason `nothing` and `unmeasured` are separate states.',
  {
    device_id: 'dev_7',
    os_image: known('sha256:aaaa'),
    policy: unmeasured('the runtime is not answering'),
    embodiment: known('sha256:cccc'),
  })

attestCase('a-guest-tier-robot-has-no-os-image',
  '§1: guest runs "a container beside the vendor stack". There is no Skynet OS image to measure '
  + 'and reporting one would be inventing a digest — so `nothing` is the true answer and the '
  + 'report is complete.',
  {
    device_id: 'dev_7',
    os_image: nothing(),
    policy: known('sha256:bbbb'),
    embodiment: known('sha256:cccc'),
  })

attestCase('a-body-that-has-not-come-up',
  '§27’s `kernel: down`. Reporting a body digest here would attest joint ratings nothing is '
  + 'applying, which is the failure the whole descriptor chain exists to prevent.',
  {
    device_id: 'dev_7',
    os_image: known('sha256:aaaa'),
    policy: nothing(),
    embodiment: nothing(),
  })

attestCase('an-unpaired-robot-still-reports',
  'No device id, because nothing has issued one. The sentence names "This robot" rather than '
  + 'inventing an identifier — and it is still produced, because a robot bringing itself up needs '
  + 'to be able to say what it is running before anybody has paired it.',
  {
    os_image: known('sha256:aaaa'),
    policy: nothing(),
    embodiment: known('sha256:cccc'),
  })

attestCase('an-unmeasured-digest-with-no-reason-given',
  'The branch a caller reaches by constructing the state without the sentence. Rendered as the '
  + 'absence it is rather than as an empty pair of brackets.',
  {
    device_id: 'dev_7',
    os_image: unmeasured(),
    policy: known('sha256:bbbb'),
    embodiment: known('sha256:cccc'),
  })

attestCase('three-holes-and-still-a-sentence',
  '§13’s rule, applied to a report rather than to a recording: the one most likely to have a hole '
  + 'in it is the one covering whatever made the hole. A check-in withheld for want of a digest is '
  + 'a robot the fleet believes is offline, which turns a named gap into a silence nobody can tell '
  + 'from a dead machine.',
  {
    device_id: 'dev_7',
    os_image: unmeasured('the slot could not be read'),
    policy: unmeasured(),
    embodiment: unmeasured('no descriptor is loaded'),
  })

attestCase('a-known-measure-with-nothing-in-it',
  'A caller bug, and the rendering is what stops it becoming a lie: an empty pair of backticks is '
  + 'read as a digest, and this reads as the absence it is. The report is still complete, because '
  + 'the caller said it measured — this file will not overrule a measurement it did not take.',
  {
    os_image: known(''),
    policy: nothing(),
    embodiment: nothing(),
  })

vocabulary('code_faults',
  'Why eight characters are not a pairing code, in the order the checks run. The order is the '
  + 'content: a credential is recognised before a length, which is what keeps `skd_x` from being '
  + 'reported as three missing characters.',
  CODE_FAULTS)
vocabulary('device_scopes',
  '§17’s and §57’s device-facing routes, one scope each. This edition’s reading and a default '
  + 'rather than a derived constant — the Hub has declared no vocabulary. The **direction** is not '
  + 'a default: it is an allow-list, and §19’s "assumed lost" is why.',
  DEVICE_SCOPES)
vocabulary('key_bindings', 'Where a device private key lives (§10).', KEY_BINDINGS)
vocabulary('identity_findings',
  'Everything §10 can find about a robot’s identity, in the order the checks run.',
  IDENTITY_FINDINGS)
vocabulary('measure_states',
  'The three states one of §10’s three digests can be in. `nothing` and `unmeasured` are two '
  + 'entries for §37.1’s reason: two facts with two different people at the end of them.',
  MEASURE_STATES)

// ── §50's birth certificate ──────────────────────────────────────────────────
//
// The corpus this contract most needs, because §50's whole value is that two parties who have
// never met agree about what one document proves. A vendor signs at manufacture; a Hub reads the
// same certificate at pairing, months later and on another continent, and an operator reads it
// again from a check-in. Three readers, and a disagreement between any two of them is a robot
// whose identity means one thing in the factory and another in the warehouse.
//
// The findings are frozen with their **sentences**, not only their codes, for the reason §50
// gives its own findings: the remedies are three different phone calls, and an edition that agreed
// about every code while paraphrasing the messages would send people to the wrong one.

function birthCase(
  id: string,
  note: string,
  certificate: BirthCertificate,
  observed: BirthObserved,
): void {
  add('birth', 'certificate', {
    id,
    note,
    input: { certificate, observed },
    expected: canonical(shapeAttested(checkBirth(certificate, observed))),
  })
}

const DEVICE_KEY = '79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664'
const OTHER_KEY = '0000000000000000000000000000000000000000000000000000000000000001'
const BODY_DIGEST = '29fe4b8d49ec7c32b6936b2e25c3cbd7b7b96c8bb21ad8de5dce3e44f8d2e543'

const CERTIFICATE: BirthCertificate = {
  device_key: DEVICE_KEY,
  model: 'G1',
  serial: 'SN-0001',
  descriptor_digest: BODY_DIGEST,
  hardware_revision: 'rev-C',
  manufactured: '2026-03-14',
  calibration_digests: ['cal-1'],
  claims_ownership: [],
  signature: 'verified',
}

const MACHINE: BirthObserved = {
  device_key: DEVICE_KEY,
  serial: 'SN-0001',
  descriptor_digest: BODY_DIGEST,
  hardware_revision: 'rev-C',
}

birthCase('a-signed-certificate-over-this-machine',
  'The only case with no findings at all, and the one that has to hold or every other case is '
  + 'measuring a function that complains about everything. A vendor signed this key, and every '
  + 'field the document binds is a field the machine confirms.',
  CERTIFICATE, MACHINE)

birthCase('unsigned-and-matching-everything',
  '§50.3, and the trap: every field below the signature "passes". Comparing a machine against a '
  + 'document nobody signed is comparing it to a story it told itself, so `attests` is false '
  + 'however well it matches — and `unattested` is first, because the document being unbelievable '
  + 'outranks any field of it agreeing.',
  { ...CERTIFICATE, signature: 'absent' }, MACHINE)

birthCase('signed-by-a-key-nobody-here-trusts',
  'Distinct from unsigned, and the reason `SignatureState` is five states rather than a boolean: '
  + 'the remedy is a trust-store change and not a phone call to the vendor. Both void the '
  + 'certificate; they do not send the same person anywhere.',
  { ...CERTIFICATE, signature: 'untrusted' }, MACHINE)

birthCase('a-replaced-mainboard',
  '§50.4: this is the expected result and not a fault. The secure element holds another key, so '
  + 'every other field binds to a machine this is not — the document is about a different robot, '
  + 'and the remedy is a vendor-signed re-issue that also invalidates the calibrations a board '
  + 'swap could have disturbed (§45.4).',
  CERTIFICATE, { ...MACHINE, device_key: OTHER_KEY })

birthCase('a-stale-serial',
  'The contrast with the case above, and the whole reason `voidsTheCertificate` is true for two '
  + 'findings rather than all of them: the key proves this document is about this machine, so a '
  + 'serial that disagrees narrows what is attested without removing it.',
  CERTIFICATE, { ...MACHINE, serial: 'SN-0002' })

birthCase('a-descriptor-this-vendor-never-attested',
  '§50.3’s field, and the gap §5’s `verified` flag cannot see: a descriptor can be perfectly well '
  + 'signed and be for a different machine. This is the only check that notices, and what it '
  + 'notices is that the joint ratings in force are ones nobody signed **for this body**.',
  CERTIFICATE, { ...MACHINE, descriptor_digest: 'f'.repeat(64) })

birthCase('a-vendor-asserting-ownership',
  '§50.2: the vendor attests what this robot is and has no business asserting whose it is. The '
  + 'machine is fine and the document overreached, so `attests` stays true — and the assertion is '
  + 'reported rather than silently dropped, because silently ignoring a field means the next '
  + 'vendor adds it too.',
  { ...CERTIFICATE, claims_ownership: ['namespace', 'operator'] }, MACHINE)

birthCase('a-robot-early-in-boot',
  '§7.2: an absent input makes a check unavailable, never passed. Nothing contradicted the '
  + 'certificate and nothing confirmed it either, and the four unchecked bindings are named — '
  + 'because the one thing worse than reporting nothing is reporting "matches" for a comparison '
  + 'nobody made.',
  CERTIFICATE, {})

birthCase('an-element-that-could-not-be-read',
  '"I could not read the element" and "the element holds another key" are different facts and '
  + 'only the second is about a different machine. This one does not void the certificate.',
  CERTIFICATE, { ...MACHINE, device_key: null })

birthCase('a-certificate-binding-nothing-against-a-machine-reporting-nothing',
  'The bug this shape prevents: an empty serial compared against a machine reporting an empty '
  + 'serial would "match", and a certificate that bound nothing would confirm everything. An '
  + 'unbound field is incomplete at any observation.',
  { ...CERTIFICATE, serial: '' }, { ...MACHINE, serial: '' })

birthCase('the-unsigned-constructor',
  'Every required field reported plus the signature, which is what a certificate somebody '
  + 'constructed and never filled in should read as. Five incomplete findings and not six: '
  + '`manufactured` is provenance the module never compares to anything.',
  unsignedCertificate('absent'), MACHINE)

vocabulary('birth_findings',
  'Everything §50 can find about a certificate, in the order of their cost. The first is about '
  + 'the document rather than the machine, which is the order a reader needs.',
  BIRTH_FINDINGS)
vocabulary('birth_required_fields',
  '§50.2’s bindings, in the order that section lists them. Written down once so that a decoder '
  + 'and whatever eventually issues a certificate cannot disagree about which fields are required.',
  BIRTH_REQUIRED_FIELDS)
vocabulary('birth_ownership_fields',
  'What a birth certificate must never carry. Ownership is established by pairing, by a different '
  + 'party (§50.2).',
  BIRTH_OWNERSHIP_FIELDS)

// ── §56's provenance classes ──────────────────────────────────────────────────
//
// The corpus §56 most needs, because unlike every other contract here the **Hub** is the consumer:
// §56.2 says the Hub assigns the class, and a robot's edition computes the same class only so it
// can predict its own offline. Two implementations of an anti-gaming rule that disagree is an
// anti-gaming rule with a hole, and the hole is always in the direction of admitting something.
//
// The order of the checks is what is really frozen here. Only one class is worth lying to obtain.

function provenanceCase(
  id: string,
  note: string,
  bundle: RunBundle,
  expectedBenchmark = BENCHMARK,
  expectedSeeds: string[] = SEEDS,
): void {
  add('provenance', 'classify', {
    id,
    note,
    input: { bundle, expected_benchmark: expectedBenchmark, expected_seeds: expectedSeeds },
    expected: canonical(shapeProvenance(classify(bundle, expectedBenchmark, expectedSeeds))),
  })
}

const BENCHMARK = 'sha256:bench'
const SEEDS = ['1', '2', '3']

const FIELD_RUN: RunBundle = {
  benchmark_digest: BENCHMARK,
  policy_digest: 'sha256:policy',
  embodiment_digest: 'sha256:body',
  os_image_digest: 'sha256:image',
  seeds: SEEDS,
  blackbox_segment: 'sha256:segment',
  tier: 'certified',
  enforcement: 'kernel',
  simulator: false,
  device_signature: 'verified',
  device_known: true,
  birth_attested: true,
  image_attested: true,
}

provenanceCase('a-field-result',
  'All four of §56.2’s conditions, the benchmark’s own seeds, and a black box segment behind it. '
  + 'The case that has to hold or every other one is measuring a function that refuses everything.',
  FIELD_RUN)

provenanceCase('an-unsigned-bundle-is-unattested-before-anything-else-is-read',
  '§56.1: a number nobody can attribute is not evidence. Every other field in this bundle is '
  + 'perfect and none of them is consulted — which is the point, because they are all claims the '
  + 'bundle makes about itself.',
  { ...FIELD_RUN, device_signature: 'absent' })

provenanceCase('a-signature-that-verifies-from-a-device-nobody-registered',
  'Nothing is wrong with the bytes; what is missing is a registered machine to attribute them to. '
  + 'A separate reason from an unsigned bundle because it is a separate thing to go and do.',
  { ...FIELD_RUN, device_known: false })

provenanceCase('a-declared-simulator-is-simulated-however-perfect-it-is',
  '§35.3 puts the obligation to declare on the driver and this is where it pays. Everything else '
  + 'about this run is field-grade, and it is still not a field result — a simulator on the field '
  + 'leaderboard is the single outcome the whole scheme exists to prevent.',
  { ...FIELD_RUN, simulator: true })

provenanceCase('an-unsigned-simulator-is-unattested-and-not-simulated',
  'The order that must not be rearranged for tidiness. The simulator flag is one of the bundle’s '
  + 'own claims, so a bundle nobody can attribute is `unattested` **before** the flag is read — '
  + 'and it is `unattested` rather than `simulated`, which is the stricter of the two.',
  { ...FIELD_RUN, simulator: true, device_signature: 'failed' })

provenanceCase('a-robot-whose-birth-certificate-attests-nothing',
  '§50 read from §56’s side: without it, “a real robot of this model with these joint ratings” '
  + 'is a claim the machine makes about itself, and a leaderboard built on that is measuring a '
  + 'story. Recorded, not ranked as field.',
  { ...FIELD_RUN, birth_attested: false })

provenanceCase('an-unattested-image',
  'What produced these numbers is not established. The fourth of §56.2’s conditions, and the one '
  + 'a publisher is most likely to think does not matter.',
  { ...FIELD_RUN, image_attested: false })

provenanceCase('three-of-the-four-conditions',
  '§56.2 has three classes and not four. A bundle that meets three conditions has not established '
  + 'that it is a field result, and `unattested` is exactly what that means — a fourth class would '
  + 'be a leaderboard tier for nearly-evidence.',
  { ...FIELD_RUN, birth_attested: false, image_attested: false })

provenanceCase('seeds-the-robot-chose-for-itself',
  '§56.3: seeds come from the benchmark and are never generated by the robot, because a robot that '
  + 'picks its own picks easy ones — and it need not do so dishonestly for the number to be '
  + 'worthless. This is not a run of that benchmark at all.',
  { ...FIELD_RUN, seeds: ['7', '8', '9'] })

provenanceCase('the-benchmarks-seeds-in-a-different-order',
  'Order as well as membership. Two runs over the same seeds in a different order are two '
  + 'different sequences of episodes, and a benchmark that fixes its seeds is fixing the sequence.',
  { ...FIELD_RUN, seeds: ['3', '2', '1'] })

provenanceCase('a-run-of-a-different-benchmark',
  'Two benchmarks are two questions, and a score against one says nothing about the other. Caught '
  + 'here rather than by a reviewer, because §56.1’s whole point is that a reviewer reading a '
  + 'number cannot see any of this.',
  { ...FIELD_RUN, benchmark_digest: 'sha256:other' })

provenanceCase('attested-and-unreplayable',
  '§56.4 is the one observation that never changes the class: a result can be perfectly attested '
  + 'and have no recording behind it. Still `field`, and still reported — because a number with a '
  + 'replayable recording is a different class of claim from a number in a table.',
  { ...FIELD_RUN, blackbox_segment: null })

// §56.5's roll-up comparison.

function scoresCase(id: string, note: string, onRobot: number, onHub: number): void {
  add('provenance', 'scores', {
    id,
    note,
    input: { scores: { on_robot: onRobot, on_hub: onHub } },
    expected: canonical(shapeScores(onRobot, onHub)),
  })
}

scoresCase('identical', 'The ordinary case: both sides ran the same scoring contract over the same '
  + 'per-episode outcomes and got the same number.', 0.875, 0.875)

scoresCase('within-the-representation-tolerance',
  'The tolerance exists for the floating point representation and not for the arithmetic. A '
  + 'difference this small is two spellings of one number.',
  0.1 + 0.2, 0.3)

scoresCase('a-real-disagreement',
  '§56.5: a defect worth investigating rather than a rounding difference to paper over. It means '
  + 'the shared contract’s two implementations diverged, which is precisely what §31.4 exists to '
  + 'catch — and the sentence says the Hub’s number is the authoritative one.',
  0.875, 0.8751)

scoresCase('a-difference-just-outside-the-tolerance',
  'The boundary, frozen. A caller tempted to widen `1e-9` is a caller papering over the divergence '
  + 'this comparison exists to surface.',
  1, 1 + 1e-8)

scoresCase('two-robots-that-computed-nothing',
  'A non-finite score is not a score, and two of them are not an agreement. Returning true here '
  + 'would report a robot and a Hub that both failed to compute anything as being in perfect '
  + 'accord.',
  Number.NaN, Number.NaN)

vocabulary('provenance_classes',
  '§56.2’s three classes, strongest to weakest. The order is the ranking, and an edition that '
  + 'sorted these alphabetically would put `field` after `unattested`.',
  PROVENANCE_CLASSES)
vocabulary('provenance_reason_codes',
  'Everything a classification can carry, in the order the checks run.',
  PROVENANCE_REASON_CODES)

// ── §11 and §17's check-in ───────────────────────────────────────────────────
//
// Three files, because a check-in has three separable ways to be wrong and an edition could agree
// about any one of them while disagreeing about the others. `claim` is the field §17's rule lives
// on — the platform never claims a robot is running something it has not heard from — `health` is
// §33.6's age rule, and `document` is the bytes that actually cross to the Hub.

function claimCase(id: string, note: string, progress: Progress): void {
  add('checkin', 'claim', {
    id,
    note,
    input: progress as unknown as Record<string, unknown>,
    expected: canonical(shapeClaimVerdict(claimFor(progress))),
  })
}

claimCase('applied',
  'An empty plan on the generation the Hub asked for. The only state in which `applied` is true, '
  + 'and the case that has to hold or §17’s rule has made the field useless rather than honest.',
  { observed_generation: 41, desired_generation: 41, pending: [] })

claimCase('converging',
  'Work left and somebody doing it. The generation on the wire is 40 — what this robot is actually '
  + 'running — and not the 41 it was asked for, which is the single field §17’s rule lives or dies '
  + 'on.',
  {
    observed_generation: 40,
    desired_generation: 41,
    pending: [{ stage: 'policy', deferred: false, reason: 'the package is staged and not started' }],
  })

claimCase('deferred',
  '§33.4’s barrier, and the reason this is a claim of its own rather than a flavour of converging: '
  + 'nothing here happens until somebody puts the robot in a safe state. An operator reading '
  + '"converging" would wait; an operator reading this one goes and does something.',
  {
    observed_generation: 40,
    desired_generation: 41,
    pending: [
      { stage: 'os_image', deferred: true, reason: 'applying an image reboots' },
      { stage: 'rhal_driver', deferred: true, reason: 'changing the driver changes the body' },
    ],
  })

claimCase('one-action-still-moving-is-not-deferred',
  'Two actions, one of them waiting and one of them not. `deferred` requires **every** pending '
  + 'action to be waiting — otherwise a robot that is actively converging would report itself '
  + 'stuck, and the one report that should send somebody to the robot would be the one they learn '
  + 'to ignore.',
  {
    observed_generation: 40,
    desired_generation: 41,
    pending: [
      { stage: 'os_image', deferred: true, reason: 'applying an image reboots' },
      { stage: 'package', deferred: false, reason: 'fetching' },
    ],
  })

claimCase('an-empty-plan-that-closes-nothing',
  'The state worth having a name for. The reconciler emitted no actions and the generations still '
  + 'disagree, which is a supervisor that cannot see the difference it exists to close. Reported '
  + 'as `applied` it would be a green tick over a bug in the one place nobody looks.',
  { observed_generation: 40, desired_generation: 41, pending: [] })

claimCase('stages-are-sorted-and-said-once',
  'Three pending actions across two stages, one repeated. `blocking` is what a fleet console groups '
  + 'by, so a duplicate renders as two problems and an unsorted list renders differently on two '
  + 'editions.',
  {
    observed_generation: 1,
    desired_generation: 2,
    pending: [
      { stage: 'policy', deferred: false, reason: 'a' },
      { stage: 'os_image', deferred: false, reason: 'b' },
      { stage: 'policy', deferred: false, reason: 'c' },
    ],
  })

claimCase('a-robot-nobody-has-told-anything',
  'No generations and no plan, which is a robot that has never reached a Hub. Zero against zero is '
  + '`applied`, and that is correct rather than a flattering default: this robot has applied '
  + 'everything it was asked for, which was nothing.',
  {})

claimCase('deferred-on-a-generation-it-already-has',
  'Waiting for a safe state while the generations agree — which happens when the desired state '
  + 'changed and changed back, or when an action is deferred that the plan will drop next pass. '
  + 'The plan is not empty, so it is not applied, and the pending list is what says why.',
  {
    observed_generation: 41,
    desired_generation: 41,
    pending: [{ stage: 'profile', deferred: true, reason: 'a widening waits for a restart' }],
  })

// ── §33.6's health, and the rule about ages ──────────────────────────────────

function healthCase(id: string, note: string, health: HealthInput): void {
  add('checkin', 'health', {
    id,
    note,
    input: health as unknown as Record<string, unknown>,
    expected: canonical(shapeHealthReport(summarize(health))),
  })
}

healthCase('a-robot-nobody-has-measured',
  'Every aggregate field absent, no packages, and **not degraded**. "We could not measure the '
  + 'battery" is not "the battery is fine" and it is also not "the robot is unhealthy" — reporting '
  + 'a gap as a fault is how an operator learns to ignore the health screen.',
  {})

healthCase('alive-and-producing-nothing',
  'The finding §33.6 exists for, and the reason liveness and readiness are two signals: a dead '
  + 'process is obvious and a live-but-stalled one looks fine on every dashboard while the robot '
  + 'stands still.',
  {
    packages: [
      { id: 'acme/grasp', live: true, readiness: 'stalled', age_ms: 40 },
      { id: 'acme/nav', live: true, readiness: 'ready', age_ms: 12 },
    ],
  })

healthCase('a-first-cycle-is-not-a-stall',
  'A policy that has legitimately produced nothing yet. Reporting `unknown` as a stall would make '
  + 'every start look like a fault, which is the same over-reporting the case above guards against '
  + 'from the other side.',
  { packages: [{ id: 'acme/grasp', live: true, readiness: 'unknown', age_ms: 5 }] })

healthCase('a-reading-with-no-age',
  '§33.6 in four words: "Every field carries its age." A battery reading nobody can date is one a '
  + 'console renders as current, and a four-hour-old 12% is exactly the number somebody drives out '
  + 'on. It is dropped, and named in `undated` rather than in `unmeasured`, because "nobody '
  + 'measured this" and "somebody measured this and lost track of when" send different people to '
  + 'different places.',
  { battery: { value: 0.12 }, storage_headroom: { value: 0.4, age_ms: 900 } })

healthCase('an-age-with-no-reading',
  'The other half of the pair, and it is simply unmeasured: an age for a value nobody took is not '
  + 'a measurement missing its date, it is a date missing its measurement.',
  { battery: { age_ms: 900 } })

healthCase('every-field-measured',
  'All five of §33.6’s aggregate fields with a value and an age. The case that pins the order they '
  + 'are emitted in, which is the document’s order and not alphabetical.',
  {
    generation_lag: { value: 0, age_ms: 1200 },
    guard_trip_rate: { value: 0.5, age_ms: 60_000 },
    storage_headroom: { value: 0.42, age_ms: 900 },
    battery: { value: 0.88, age_ms: 2_000 },
    temperature_c: { value: 51.5, age_ms: 3_000 },
  })

healthCase('a-negative-age-is-not-an-age',
  'A clock that went backwards, or a subtraction done in the wrong order. Treated as undated '
  + 'rather than passed through, because a reading dated in the future is worse than one that is '
  + 'not dated at all — the second is visibly missing and the first looks fresh.',
  { battery: { value: 0.5, age_ms: -1 } })

healthCase('a-lag-of-zero-is-a-measurement',
  'The case a naive falsy check breaks. Zero generation lag is a robot that is up to date, and it '
  + 'must not read as a field nobody measured — which is the same distinction §33.6 draws between '
  + 'zero guard trips and no measurement of guard trips.',
  { generation_lag: { value: 0, age_ms: 500 } })

// ── The document ─────────────────────────────────────────────────────────────

function documentCase(id: string, note: string, input: CheckInInput): void {
  add('checkin', 'document', {
    id,
    note,
    input: input as unknown as Record<string, unknown>,
    expected: canonical(shapeCheckIn(checkIn(input))),
  })
}

const CONVERGED: CheckInInput = {
  device_id: 'dev_7',
  agent: 'skynetd/0.1.0',
  contracts: '1.0.0',
  state: 'running',
  desired_digest: 'sha256:dddd',
  progress: { observed_generation: 41, desired_generation: 41, pending: [] },
  health: {
    packages: [{ id: 'acme/grasp', live: true, readiness: 'ready', age_ms: 12 }],
    storage_headroom: { value: 0.42, age_ms: 900 },
  },
  attestation: {
    device_id: 'dev_7',
    os_image: { state: 'known', digest: 'sha256:aaaa' },
    policy: { state: 'known', digest: 'sha256:bbbb' },
    embodiment: { state: 'known', digest: 'sha256:cccc' },
  },
}

documentCase('a-healthy-robot-on-the-generation-it-was-given',
  'The bytes that cross to the Hub when everything is right. Frozen whole, because two editions '
  + 'producing different check-ins for the same robot would show a fleet console two robots that '
  + 'differ only in who compiled their supervisor.',
  CONVERGED)

documentCase('waiting-for-a-safe-state',
  'The same robot, mid-update, with §33.4’s barrier in the way. `applied` is false, the claim says '
  + 'which kind of false, and the pending list names the stage and the reason — because "the update '
  + 'did not take" and "the robot has not been put in a safe state" send an engineer to different '
  + 'places.',
  {
    ...CONVERGED,
    state: 'hold',
    progress: {
      observed_generation: 40,
      desired_generation: 41,
      pending: [{ stage: 'os_image', deferred: true, reason: 'applying an image reboots' }],
    },
  })

documentCase('a-robot-that-can-measure-almost-nothing',
  'An unpaired, freshly booted robot with no descriptor, no policy and nothing measured. The '
  + 'check-in is still produced and every hole in it is named — §13’s rule applied to a report, '
  + 'because the robot most in need of one is the robot that cannot measure everything.',
  {
    agent: 'skynetd/0.1.0',
    contracts: '1.0.0',
    state: 'post',
    progress: {},
    health: {},
    attestation: {
      os_image: { state: 'unmeasured', why: 'no Skynet OS image is reported' },
      policy: { state: 'nothing' },
      embodiment: { state: 'nothing' },
    },
  })

documentCase('a-robot-that-holds-no-desired-state',
  'Paired, converged on generation 0, and reconciling against nothing — a robot the Hub has minted '
  + 'a credential for and never told what to be. The digest is empty rather than absent, because '
  + '"this robot holds no document" is a fact a fleet page acts on and an absent key is one it has '
  + 'to guess at.',
  {
    ...CONVERGED,
    desired_digest: '',
    progress: { observed_generation: 0, desired_generation: 0, pending: [] },
  })

documentCase('the-limits-a-robot-is-under',
  '§7.6 crossing §17. The robot resolves its own ceilings and guard partition and until this field '
  + 'existed nothing carried them: §32.1 asks a device page for exactly this and the Hub’s own '
  + 'route answered with a `gaps` list naming it. Frozen because the two halves of the sentence an '
  + 'operator reads — which guards run, and whether anything enforces them — are in '
  + 'different fields, and an edition that agreed about one could disagree about the other.',
  {
    ...CONVERGED,
    limits: {
      resolved_at: 'guest',
      enforcement: 'advisory',
      // False at the guest tier and false at every other one on an edition with no policy runtime.
      // The pair of fields is the case: `enforcement` says what the tier permits and this says
      // whether anything does it.
      enforced: false,
      joint_velocity_max_rad_s: 0.5,
      joint_torque_max_nm: 80,
      base_velocity_max_m_s: null,
      min_control_hz: null,
      max_delta_per_cycle: null,
      source: 'vendor datasheet rev C',
      active: ['watchdog', 'envelope', 'mode_gate'],
      unavailable: [
        { guard: 'rate_limit', missing: ['profile.max_delta_per_cycle'] },
        // The row that has to survive verbatim: `balance` on a fixed base is not a defect, and
        // §7.2 spends a paragraph on saying so rather than reporting a missing IMU.
        { guard: 'balance', missing: ['a legged base — not applicable to this body'] },
      ],
      notices: [
        { code: 'advisory_only', subjects: [] },
        { code: 'no_independent_stop', subjects: [] },
        { code: 'guard_unavailable', subjects: ['profile.max_delta_per_cycle'] },
      ],
    },
  })

documentCase('a-robot-that-resolves-no-limits',
  'The same document from an edition that does not compose them. `limits` is present and null, '
  + 'because "this robot reports no limits" and "this robot has no ceilings" are different facts '
  + 'and a consumer switching on a missing key cannot tell them apart — the second is a report '
  + 'whose numbers are all null and whose notices say `no_envelope`.',
  { ...CONVERGED, limits: undefined })

documentCase('a-stalled-policy-on-a-converged-robot',
  'Applied, and degraded. The two are independent and a document that collapsed them would hide '
  + 'the case §33.6 spends a paragraph on: a robot that reconciled perfectly and is standing still.',
  {
    ...CONVERGED,
    health: {
      packages: [{ id: 'acme/grasp', live: true, readiness: 'stalled', age_ms: 4_000 }],
      storage_headroom: { value: 0.42, age_ms: 900 },
    },
  })

vocabulary('checkin_claims',
  'What a robot may claim about the generation it was given. Four rather than §17’s boolean, '
  + 'because "not applied" covers a robot that is working, one that is waiting for a person, and '
  + 'one whose supervisor disagrees with itself.',
  CLAIMS)
vocabulary('reconcile_stages',
  '§33.4’s six ordered stages, as a check-in names them. Frozen because a fleet console groups by '
  + 'them, and six spellings of one stage is six columns.',
  STAGES)
vocabulary('readiness_states',
  '§33.6’s four. `stalled` and `unknown` are the pair that carries the section: one is a policy '
  + 'that has stopped producing and the other is one that has not started.',
  READINESS)
vocabulary('health_fields',
  'The aggregate health fields, in the order §33.6 lists them. Every one is a key on every '
  + 'check-in whether or not it was measured, so a console iterates the set rather than '
  + 'discovering which of them a robot happened to have.',
  HEALTH_FIELDS)

// ── §11's pull ───────────────────────────────────────────────────────────────
//
// One file, because there is one function — and every case in it is really a case about a single
// question: may a response from a network become the file that decides what software runs on a
// machine that moves. Two of the answers are "no, and the robot carries on", which §34.1 makes the
// ordinary state rather than a failure.

function offerCase(id: string, note: string, offer: Offer, held: Held): void {
  add('intent', 'offer', {
    id,
    note,
    input: { offer, held },
    expected: canonical(shapeOfferVerdict(assessOffer(offer, held))),
  })
}

const HELD: Held = { generation: 41, digest: 'sha256:aaaa', running: 2 }

const good = (generation: number, digest: string, packages: number): Offer =>
  ({ decoded: true, generation, digest, packages })

offerCase('the-ordinary-poll',
  'Most polls. §11’s loop asks the same question every few seconds and the answer is usually the '
  + 'document the robot already has — so `adopt` is true and `changed` is false, and a caller that '
  + 'could not tell the two apart would rewrite the desired state on every tick.',
  good(41, 'sha256:aaaa', 2), HELD)

offerCase('a-new-generation',
  'What the loop exists for. Adopted, and it changes something.',
  good(42, 'sha256:bbbb', 3), HELD)

offerCase('not-a-desired-state',
  '§33.8 says a robot never invents a desired state; this is the step before that, where a response '
  + 'is stopped from *becoming* one. The robot carries on against what it has, which is what it '
  + 'does whenever it cannot reach the Hub at all — so a garbage response costs it nothing that '
  + 'being offline does not.',
  { decoded: false, decode_error: '`generation` is absent, and it is what makes a check-in idempotent' },
  HELD)

offerCase('a-response-that-decoded-and-said-nothing-about-why',
  'The branch a caller reaches by reporting a failure without a reason. The sentence still works '
  + 'rather than trailing off after a colon.',
  { decoded: false }, HELD)

offerCase('a-reused-generation',
  'The case this file was written for. A Hub serving two documents under one generation has broken '
  + 'the property §11 rests idempotence on, and the robot is the only party that can see it. '
  + 'Refused, because adopting would have the robot running new software while reporting the old '
  + 'generation applied — and a fleet console would show it converged, which is exactly the claim '
  + '§17 forbids and is invisible because every robot looks right.',
  good(41, 'sha256:bbbb', 2), HELD)

offerCase('a-generation-that-went-backwards',
  'The opposite call, deliberately. A Hub restored from a backup serves a lower counter, and a '
  + 'robot that refused to follow one would be a fleet nobody could recover without visiting every '
  + 'machine. Adopted, and reported once.',
  good(40, 'sha256:bbbb', 2), HELD)

offerCase('a-desired-state-that-empties-a-running-robot',
  'An operator emptying a robot and a Hub serving a stub are indistinguishable from here. Adopted, '
  + 'because stopping is never the dangerous direction (§33.4), and said out loud because the two '
  + 'readings send different people to different places.',
  good(42, 'sha256:bbbb', 0), HELD)

offerCase('emptying-a-robot-that-is-running-nothing',
  'The same document against a robot with nothing running. Not news, and not reported — a finding '
  + 'that fired on every idle robot would be one nobody reads.',
  good(42, 'sha256:bbbb', 0), { generation: 41, digest: 'sha256:aaaa', running: 0 })

offerCase('the-first-pull-this-robot-has-ever-made',
  'Nothing held, so there is no digest to compare and no generation to have gone backwards. It '
  + 'must not report a reuse, which is what a naive comparison against an empty digest would do — '
  + 'and it must report `changed`, because everything is a change to a robot that had nothing.',
  good(0, 'sha256:aaaa', 1), {})

offerCase('both-warnings-at-once',
  'A backup restore that also emptied the robot. Both are reported and both are adopted; the '
  + 'ordering of the list is frozen so an operator gets one list rather than one per poll.',
  good(40, 'sha256:bbbb', 0), HELD)

offerCase('a-generation-of-zero-against-a-robot-that-has-applied-one',
  'The boundary, and the case a falsy check breaks. Zero is a generation, so this is intent going '
  + 'backwards rather than a field nobody set.',
  good(0, 'sha256:bbbb', 2), HELD)

vocabulary('offer_faults',
  'Everything §11’s pull can find wrong with a desired state it was handed, in the order the checks '
  + 'run. Two refuse and two report, and which is which is the content of the set.',
  OFFER_FAULTS)


// ── §74's telemetry ──────────────────────────────────────────────────────────

function windowCase(id: string, note: string, input: TelemetryWindow): void {
  add('telemetry', 'window', {
    id,
    note,
    input: input as unknown as Record<string, unknown>,
    expected: canonical(shapeWindowReport(assessWindow(input))),
  })
}

function coverageCase(id: string, note: string, windows: TelemetryWindow[]): void {
  add('telemetry', 'coverage', {
    id,
    note,
    input: { windows } as unknown as Record<string, unknown>,
    expected: canonical(shapeCoverage(coverage(windows))),
  })
}

/** Five minutes, wall time and monotonic agreeing. */
const FIVE_MINUTES: TelemetryWindow = {
  from_ms: 1_760_000_000_000,
  to_ms: 1_760_000_300_000,
  elapsed_ms: 300_000,
  counters: { cycles: 30_000, guard_trips: 2, interventions: 1 },
}

windowCase('an-ordinary-five-minute-window',
  'The bytes that cross when everything is right. Rates are per second and over the monotonic '
  + 'duration, and the five counters nothing measured are named rather than reported as zero.',
  FIVE_MINUTES)

windowCase('a-window-that-measured-nothing',
  'A robot with no kernel running: five minutes of being up, and nothing counted. It is a valid '
  + 'window — "the robot was up for five minutes and counted nothing" is a fact — and it is what '
  + 'every robot this edition runs will send.',
  { ...FIVE_MINUTES, counters: {} })

windowCase('a-window-that-counted-zero',
  'The distinction §74.3 exists for, beside the case above. Zero guard trips and no measurement of '
  + 'guard trips render identically as 0, and the second is the number an operator relaxes on.',
  { ...FIVE_MINUTES, counters: { guard_trips: 0 } })

windowCase('a-window-with-no-duration',
  'Nothing to divide by, so no rate is anything. Refused rather than counted as zero — a rate of '
  + 'zero over no time is the most confident wrong number a fleet page can render.',
  { ...FIVE_MINUTES, elapsed_ms: 0 })

windowCase('a-window-that-does-not-say-when-it-was',
  'Unbounded. A Hub cannot place it beside another robot’s, and the gap between windows is how a '
  + 'dropped one is noticed at all (§34.4, §47.4).',
  { elapsed_ms: 300_000, counters: { cycles: 1_000 } })

windowCase('a-window-whose-clock-stepped-backwards',
  'Ends before it starts. A clock that moved under the robot rather than a counting error, and the '
  + 'counts are over an interval nobody can name.',
  { from_ms: 1_760_000_300_000, to_ms: 1_760_000_000_000, elapsed_ms: 300_000, counters: {} })

windowCase('a-machine-that-was-suspended',
  'Wall time ran an hour ahead of the monotonic clock. §74.2: the disagreement is reported and the '
  + 'counts are kept, because a suspended machine and a corrected clock look the same from here and '
  + 'neither is a reason to throw a measurement away.',
  { ...FIVE_MINUTES, to_ms: 1_760_003_900_000 })

coverageCase('two-windows-that-tile',
  'Contiguous, so there is nothing missing and the span equals what was observed.',
  [
    FIVE_MINUTES,
    { ...FIVE_MINUTES, from_ms: 1_760_000_300_000, to_ms: 1_760_000_600_000,
      counters: { cycles: 30_000, guard_trips: 0, interventions: 0 } },
  ])

coverageCase('a-dropped-window-leaves-a-gap',
  '§34.4 drops telemetry first and aggressively, and §47.4 requires the gap to be visible as a gap. '
  + 'It is: the windows stop tiling, and the Hub knows exactly which ten minutes it does not have — '
  + 'without anybody having remembered to annotate anything.',
  [
    FIVE_MINUTES,
    { ...FIVE_MINUTES, from_ms: 1_760_000_900_000, to_ms: 1_760_001_200_000,
      counters: { cycles: 30_000 } },
  ])

coverageCase('an-unusable-window-is-counted-apart',
  'One window with no duration, beside a good one. The bad one is not counted and its absence is '
  + 'reported as a count rather than being silently dropped into the same total.',
  [FIVE_MINUTES, { ...FIVE_MINUTES, elapsed_ms: 0 }])

coverageCase('overlapping-windows-are-merged-for-the-span-and-summed-for-the-counts',
  'A robot that restarted and re-reported. The honest reading is that both sets of events happened, '
  + 'and that the interval was observed once.',
  [
    FIVE_MINUTES,
    { ...FIVE_MINUTES, from_ms: 1_760_000_150_000, to_ms: 1_760_000_450_000,
      counters: { cycles: 15_000 } },
  ])

// §15's floor, from both sides. Every case above sits comfortably over 0.999, so none of them
// discriminates the number the third suite exists to produce — an edition could compute
// conformance differently and reproduce all four. These four are the ones that pin it.

coverageCase('conformance-under-the-floor',
  '§15 suspends below 0.999, "because a robot whose commands have been leaving its envelope is not '
  + 'a lower-tier robot — it is one whose certificate no longer describes it." Thirty thousand '
  + 'cycles and forty interventions is 0.99867, and this is the case the third suite exists for.',
  [{ ...FIVE_MINUTES, counters: { cycles: 30_000, interventions: 40 } }])

coverageCase('conformance-exactly-at-the-floor',
  'Thirty interventions in thirty thousand cycles is exactly 0.999. §15 puts the floor at 0.999 '
  + 'rather than 1.0 because "a hundred robots at 100 Hz produce tens of millions of cycles a day, '
  + 'and demanding literal perfection would suspend every honest vendor on sensor noise" — so the '
  + 'boundary is worth a vector of its own.',
  [{ ...FIVE_MINUTES, counters: { cycles: 30_000, interventions: 30 } }])

coverageCase('cycles-counted-and-no-interventions-counted',
  'The one place §74.3\'s unmeasured-is-not-zero rule bends, and it bends because of what the '
  + 'counters are: the thing that would have recorded an intervention is the same loop that '
  + 'recorded the cycles, so a window with cycles and no interventions is a kernel that intervened '
  + 'in none of them. Conformance is 1, not undefined.',
  [{ ...FIVE_MINUTES, counters: { cycles: 30_000 } }])

coverageCase('more-interventions-than-cycles',
  'A robot whose counters disagree with each other. Clamped to zero rather than reported negative: '
  + '§15 checks this against a floor, and a negative number sails under it — which happens to reach '
  + 'the right verdict for the wrong reason, and would reach the wrong one the day somebody '
  + 'compared it against anything else.',
  [{ ...FIVE_MINUTES, counters: { cycles: 100, interventions: 250 } }])

coverageCase('nothing-at-all',
  'No windows. Zero observed, zero spanned, no gaps and every counter unmeasured — which is a '
  + 'different answer from a robot that reported windows full of zeroes.',
  [])

vocabulary('telemetry_counters',
  '§74’s closed set of counters. Frozen because a fleet console divides by these, and six spellings '
  + 'of one counter is six columns that each look empty.',
  TELEMETRY_COUNTERS)

vocabulary('power_states',
  '§36.1\'s four, plus the fifth §36.1 does not have. Ordered most capable first, which is the '
  + 'order a console renders a fleet in — and `unmetered` is last because it is not a rung of the '
  + 'same ladder.',
  POWER_STATES)

vocabulary('power_reasons',
  'Why a robot is in the power state it is in, in rule order. The brownout override is first '
  + 'because it is the one rule that ignores a reading it has.',
  POWER_REASONS)

vocabulary('power_fault_codes',
  'Everything §36 can find wrong with the power subsystem itself, as distinct from a level of '
  + 'charge. Two of the eight raise DEGRADED and five withhold leases, and an edition that agreed '
  + 'about the states could still disagree about which — so the set is frozen alongside them.',
  POWER_FAULT_CODES)

vocabulary('thermal_pressures', '§36.3\'s three levels.', THERMAL_PRESSURES)

vocabulary('shed_order',
  '§36.3\'s ladder, weakest claim first. Order **is** the agreement here: an edition that sorted '
  + 'this alphabetically would give up the black box before the OTA download and the policy loop '
  + 'before either.',
  SHED_ORDER)

vocabulary('never_shed',
  'The three §36.3 never gives up, at any pressure. Frozen so that "never" is something a test can '
  + 'check rather than a word in a comment.',
  NEVER_SHED)

vocabulary('reserve_terms',
  '§36.2\'s three terms, in that section\'s order. Two of them have no source anywhere on this '
  + 'platform, which is why they are reported as unaccounted rather than silently omitted.',
  RESERVE_TERMS)

vocabulary('telemetry_window_faults',
  'Everything §74 can find wrong with a window. Three refuse it and one reports — the clock skew is '
  + 'a fact about the robot rather than a reason to discard a measurement.',
  WINDOW_FAULTS)

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
console.log(`fuzz/observation coverage: ${Object.entries(obsCoverage)
  .map(([key, value]) => `${key} ${value}`).join(', ')}`)
