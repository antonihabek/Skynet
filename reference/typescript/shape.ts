/**
 * The wire shape of every contract output.
 *
 * The oracle modules return TypeScript objects whose optional fields are `undefined` when absent
 * and whose nullable fields are `null`. That distinction is meaningful in the language and
 * meaningless once serialised unless somebody writes down which is which — so this file writes it
 * down, and `crates/skynet-contracts/src/json.rs` is the same decision made again in Rust.
 *
 * The rule, applied uniformly:
 *
 * * A field the schema declares **nullable** is always present, `null` when it has no value. It is
 *   a fact that there is no value: no source digest, no target joint, no attainable tier.
 * * A field the schema declares **optional** is absent when it has no value. There is nothing to
 *   say: no subjects on this warning, no torque ceiling in this clamp.
 *
 * Collapsing the two would erase the distinction the whole platform is built on — "nobody declared
 * a safety envelope" is not "the safety envelope is null".
 */

import type { FrameFault, MotionFault, TransportPlan } from './embodiment.ts'
import type { AdmissionResult } from './package.ts'
import type { CertificationVerdict } from './conformance.ts'
import type { CheckResult, FleetReadiness, ReadinessReport } from './readiness.ts'
import type { CycleVerdict, PoseOutcome, ResolvedLimits } from './safety.ts'
import { collisionPairs, type Frame, type Solved } from './kinematics.ts'
import type { DigestOutcome } from './digest.ts'
import {
  guardsFedBy, validityDetail, type GuardUnavailable, type Validity,
} from './calibration.ts'
import {
  commandsAutonomously, isSafeState, isSticky, OPERATING_STATES, warningRequired,
  type Transition,
} from './opstate.ts'
import type { LiveContext } from './live.ts'
import type { BootVerdict, BundleVerdict } from './ota.ts'
import type {
  Attestation, AttestationReport, CodeResult, IdentityVerdict, Measure,
} from './identity.ts'
import {
  attests as birthAttests, explain as birthExplain, fullyConfirmed as birthFullyConfirmed,
  unchecked as birthUnchecked, type Attested as BirthAttested,
} from './birth.ts'
import {
  blocksField, explain as provenanceExplain, rankable, rankedSeparately, replayable,
  scoresAgree, explainDisagreement, type Classified,
} from './provenance.ts'
import type { CheckIn, ClaimVerdict, GuardReport, HealthReport } from './checkin.ts'
import type { OfferVerdict } from './intent.ts'
import { conformanceOf, type Coverage, type WindowReport } from './telemetry.ts'
import { attest as attestOfMeasures } from './identity.ts'
import { HEALTH_FIELDS } from './checkin.ts'
import type { SkillTransition } from './skill.ts'
import {
  CHAIN, WINDOWS, checkNesting, resolveCommandLoss, type Windows,
} from './watchdog.ts'
import {
  assess as assessPower, budgetMissed, degraded as powerDegraded,
  grantsNewLeases, hostBlockedBySafeState, reserve as powerReserve, reserveComplete,
  safeStateBasis, stateNotifiesOperator, stateRequiresSafeState,
  stateStopsImmediately, validateSafeState,
  type PowerObservation, type PowerProfile,
} from './power.ts'
import { toRecord, type Frame as ObservationFrame } from './observation.ts'
import type { PredicateOutcome } from './predicate.ts'
import type { ParsedVersion } from './semver.ts'
import type {
  ClauseResult, CompositionFault, Deadline, PreconditionReport, SuccessJudgement,
} from './skill.ts'
import {
  cellEnvironment, cellRobotLayer, cellRobotSlugs, validateCell,
  type CellDescriptor, type CellFault,
} from './cell.ts'
import {
  relayPermissions, relaySession, type ConsoleSession,
} from './relay.ts'
import {
  admitMotion, motionProgress, type MotionGate, type MotionRequest,
} from './execute.ts'
import type { Shape as VolumeShape, WorkspaceLayer } from './kinematics.ts'

/** Present-and-null, for a nullable field. */
function nullable<T>(value: T | null | undefined): T | null {
  return value ?? null
}

/** Present-or-absent, for an optional field. */
function optional<T>(key: string, value: T | null | undefined): Record<string, T> {
  return value === undefined || value === null ? {} : { [key]: value }
}

export function shapePlan(plan: TransportPlan): unknown {
  return {
    verdict: plan.verdict,
    robot_slug: plan.robot_slug,
    source_digest: nullable(plan.source_digest),
    target_digest: nullable(plan.target_digest),
    control_mode: plan.control_mode,
    control_hz: nullable(plan.control_hz),
    bindings: plan.bindings.map(binding => ({
      source_index: binding.source_index,
      source_role: nullable(binding.source_role),
      method: binding.method,
      target: nullable(binding.target),
      scale: binding.scale,
      limit: nullable(binding.limit),
      reason: binding.reason,
    })),
    // §84.3. Absent from a plan over joint channels rather than empty, so the plans every edition
    // produced before motions existed are unchanged byte for byte.
    ...(plan.motions !== undefined && {
      motions: plan.motions.map(binding => ({
        source_motion: binding.source_motion,
        method: binding.method,
        target: nullable(binding.target),
        missing: binding.missing,
        reason: binding.reason,
      })),
    }),
    observations: plan.observations.map(observation => ({
      required: observation.required,
      method: observation.method,
      target: nullable(observation.target),
      reason: observation.reason,
    })),
    clamp: plan.clamp === null || plan.clamp === undefined ? null : {
      ...optional('joint_velocity_max_rad_s', plan.clamp.joint_velocity_max_rad_s),
      ...optional('joint_torque_max_nm', plan.clamp.joint_torque_max_nm),
      ...optional('base_velocity_max_m_s', plan.clamp.base_velocity_max_m_s),
      ...optional('min_control_hz', plan.clamp.min_control_hz),
      per_channel: plan.clamp.per_channel,
      ...optional('joint_modes', plan.clamp.joint_modes),
      ...optional('source', plan.clamp.source),
    },
    warnings: plan.warnings.map(warning => ({
      code: warning.code,
      message: warning.message,
      ...optional('subjects', warning.subjects),
    })),
  }
}

export function shapeAdmission(result: AdmissionResult): unknown {
  return {
    admitted: result.admitted,
    findings: result.findings.map(finding => ({
      code: finding.code,
      severity: finding.severity,
      message: finding.message,
      ...optional('subjects', finding.subjects),
    })),
    effective_grants: result.effective_grants,
  }
}

export function shapeCertification(verdict: CertificationVerdict): unknown {
  return {
    awarded: verdict.awarded,
    failures: verdict.failures,
    missing: verdict.missing,
    reasons: verdict.reasons,
    expires_in_days: nullable(verdict.expires_in_days),
  }
}

function shapeCheck(check: CheckResult): unknown {
  return {
    check: check.check,
    ok: check.ok,
    required_for: check.required_for,
    detail: check.detail,
  }
}

export function shapeReadiness(report: ReadinessReport): unknown {
  return {
    slug: report.slug,
    name: report.name,
    attainable: nullable(report.attainable),
    checks: report.checks.map(shapeCheck),
    blockers: report.blockers.map(shapeCheck),
    completeness: report.completeness,
  }
}

export function shapeFleet(fleet: FleetReadiness): unknown {
  return {
    total: fleet.total,
    by_tier: fleet.by_tier,
    common_blockers: fleet.common_blockers.map(entry => ({
      check: entry.check,
      robots: entry.robots,
    })),
  }
}

export function shapeLimits(limits: ResolvedLimits): unknown {
  return {
    tier: limits.tier,
    enforcement: limits.enforcement,
    joint_velocity_max_rad_s: nullable(limits.joint_velocity_max_rad_s),
    joint_torque_max_nm: nullable(limits.joint_torque_max_nm),
    base_velocity_max_m_s: nullable(limits.base_velocity_max_m_s),
    min_control_hz: nullable(limits.min_control_hz),
    max_delta_per_cycle: nullable(limits.max_delta_per_cycle),
    // §7.1's fifth guard holds at this distance. Absent from this shape until §11 could
    // deliver it, when it was always null and not worth a column; reported now, because an
    // operator who cannot see the margin in force cannot tell a robot holding 5 cm from one
    // holding at contact.
    self_collision_margin_m: nullable(limits.self_collision_margin_m),
    per_channel: limits.per_channel,
    // §86.3. Absent — not empty — on a body no joint of which is bound, so every limits document
    // written before this section is the same bytes.
    ...(Object.keys(limits.joint_modes).length > 0 && { joint_modes: limits.joint_modes }),
    active_guards: limits.active_guards,
    unavailable_guards: limits.unavailable_guards.map(entry => ({
      guard: entry.guard,
      missing: entry.missing,
    })),
    inert_guards: limits.inert_guards.map(entry => ({
      guard: entry.guard,
      reading: entry.reading,
    })),
    source: nullable(limits.source),
    basis: nullable(limits.basis),
    notices: limits.notices.map(notice => ({
      code: notice.code,
      message: notice.message,
      ...optional('subjects', notice.subjects),
    })),
  }
}

export function shapeVerdict(verdict: CycleVerdict): unknown {
  return {
    command: verdict.command,
    response: nullable(verdict.response),
    tripped: verdict.tripped,
    clamped: verdict.clamped.map(entry => ({
      channel: entry.channel,
      from: entry.from,
      to: entry.to,
    })),
    advisory: verdict.advisory,
    pose: shapePose(verdict.pose),
  }
}

/**
 * What the two pose guards found.
 *
 * Emitted on every verdict, empty or not. Omitting it when nothing tripped would make an edition
 * that has not implemented §44 byte-identical to one that ran the guards and found nothing.
 */
export function shapePose(pose: PoseOutcome): unknown {
  return {
    evaluated: pose.evaluated,
    lookahead: pose.lookahead,
    truncated: pose.truncated,
    unresolved: pose.unresolved,
    trips: pose.trips.map(trip => ({
      guard: trip.guard,
      frame: trip.frame,
      // Present and null on a workspace excursion rather than absent: "this finding is about one
      // frame" is a fact, and a reader of an absent key cannot tell it from a producer that forgot.
      other: nullable(trip.other),
      margin_m: nullable(trip.margin_m),
    })),
  }
}

/**
 * A solved pose set.
 *
 * The corpus pins this, which holds two editions to the same *arithmetic* and not only to the same
 * verdicts. A trip is a boolean and two implementations could agree on every one of them while
 * disagreeing about every pose by a millimetre; thirty digits of translation do not permit that.
 */
export function shapeSolved(frames: Frame[], solved: Solved): unknown {
  return {
    frames: frames.map((frame, index) => {
      const pose = solved.poses[index]
      return {
        name: frame.name,
        resolved: pose !== null && pose !== undefined,
        // Present and null, not absent. "This frame has no pose" is the fact the guards act on.
        translation: pose ? pose.translation : null,
        rotation: pose ? pose.rotation : null,
      }
    }),
    lookahead: solved.lookahead,
    unresolved: solved.unresolved.map(index => frames[index].name),
    collision_pairs: collisionPairs(frames),
  }
}

/**
 * A frame-graph verdict.
 *
 * `fault` is nullable rather than optional: "this graph is usable" is a fact worth stating, and a
 * consumer reading an absent key cannot tell it from a producer that forgot to run the check.
 */
/**
 * The wire shape of a motion-list verdict (§84.1), the same shape as a frame-graph verdict and for
 * the same reason: `fault` is present-and-null when the list is usable, because a consumer reading
 * an absent key cannot tell a usable list from a producer that forgot to check.
 */
export function shapeMotionFault(fault: MotionFault | null): unknown {
  return {
    fault: fault === null ? null : { code: fault.code, subjects: fault.subjects, detail: fault.detail },
  }
}

export function shapeFrameFault(fault: FrameFault | null): unknown {
  return {
    fault: fault === null ? null : { code: fault.code, subjects: fault.subjects, detail: fault.detail },
  }
}

/**
 * The wire shape of a digest outcome (§37.1).
 *
 * An outcome rather than a string that might be empty: "this package hashes to X" and "this
 * package has no computable identity" are the two answers an installer acts on differently, and a
 * caller must not be able to confuse them. `subject` names the offending thing — a path, a dotted
 * field, a key — and `detail` carries a reason only where the kind alone does not identify one.
 */
export function shapeDigest(outcome: DigestOutcome): unknown {
  if (outcome.ok) return { digest: outcome.digest, error: null }

  const error = outcome.error
  const subject =
    error.kind === 'path' || error.kind === 'duplicate_path' || error.kind === 'sha256'
      ? error.path
      : error.kind === 'number_out_of_range'
        ? error.field
        : error.kind === 'key_outside_bmp'
          ? `${error.at} :: ${error.key}`
          : null
  const detail = error.kind === 'path' ? error.detail : error.kind === 'manifest' ? error.detail : null

  return { digest: null, error: { kind: error.kind, subject, detail } }
}

/**
 * Calibration (§45): one record's verdict, and the guards it starves.
 *
 * `reason` is nullable rather than optional — a record that is valid has *no* expiry reason, which
 * is a fact worth stating rather than a field worth omitting — and `checked` is always an array,
 * empty for a record whose policy checks nothing. That empty array is load-bearing: it is how a
 * console tells "still valid" from "nothing was checked".
 */
export function shapeCalibration(
  records: Array<[string, Validity]>,
  unavailable: GuardUnavailable[],
): unknown {
  return {
    records: records.map(([set, validity]) => ({
      set,
      state: validity.state,
      detail: validityDetail(validity),
      reason: validity.state === 'expired' ? validity.reason : null,
      checked: validity.state === 'valid' ? validity.checked : [],
      guards: [...guardsFedBy(set)],
    })),
    unavailable: unavailable.map(entry => ({
      guard: entry.guard,
      set: entry.set,
      state: entry.state,
      detail: entry.detail,
    })),
  }
}

/**
 * The watchdog chain (§46): the chain, the command-loss verdict, and the nesting.
 *
 * One document rather than three, because the three are one question — *is this robot's watchdog
 * chain sound* — and a console that had to join them would be the fourth place the nesting rule was
 * written down.
 *
 * `ordered` and `complete` are both present, because they are two different facts and a single
 * boolean would have to lie about one of them: nothing contradicts, and nothing was established.
 */
export function shapeWatchdog(
  onCommandLoss: string | null, windows: Windows,
  /** §87.3's map, when the body declares one. */
  byChain?: Record<string, string> | null,
  /** The chains this body's joints are on, from `chainsOf` (§87.3). */
  chains?: readonly string[],
): unknown {
  const report = checkNesting(windows)
  // One answer about host, whichever way the body declared (§87.3). The resolver delegates to
  // `hostBlockedByCommandLoss` for the body-wide rules rather than restating them, so a document
  // over a body with no map is byte-identical to the one this function produced before §87.
  const loss = resolveCommandLoss({ body: onCommandLoss, by_chain: byChain, chains })
  return {
    chain: CHAIN.map(layer => ({
      layer: layer.layer,
      name: layer.name,
      catches: layer.catches,
      response: layer.response,
      required_from: layer.required_from,
      petted_by: layer.petted_by,
    })),
    on_command_loss: nullable(onCommandLoss),
    // §87.3. Present on every document, because *"this body declares one behaviour"* and *"this
    // body declares one behaviour per chain"* are two states a reader has to be able to tell apart
    // without knowing which fields the descriptor happened to carry.
    per_chain: {
      declared: loss.per_chain,
      chains: loss.chains.map(row => ({ chain: row.chain, behaviour: nullable(row.behaviour) })),
      unknown_chains: [...loss.unknown_chains],
    },
    host_blocked: nullable(loss.host_blocked),
    nesting: {
      // An array, not an object: canonical JSON sorts an object's keys, and these four windows
      // mean nothing except in order. The object form renders them alphabetically, which on a
      // panel about nesting is worse than rendering nothing.
      declared: WINDOWS.map(name => ({ window: name, ms: nullable(windows[name]) })),
      faults: report.faults.map(fault => ({
        code: fault.code, inner: fault.inner, outer: fault.outer, detail: fault.detail,
      })),
      missing: [...report.missing],
      ordered: report.ordered,
      complete: report.complete,
    },
  }
}

/**
 * §36's whole answer for one cycle: the state, the reserve, the thermal ladder, the faults, and
 * whether the running policy is still inside its declared budget.
 *
 * One document rather than four, for the reason §46's is one: they are one question — *what is this
 * robot's power situation right now* — and a console that had to join them would be the second
 * place §36.1's table was written down. The budget is in here rather than beside it because the
 * answer to "may this policy keep running" is exactly as much a power answer as the state is; §36.3
 * puts them in the same paragraph.
 *
 * The four booleans are derived here rather than left to a consumer, and they are four rather than
 * one because §36.1's rows differ in more than severity: `low` withholds leases and lets a running
 * task finish, `reserve` asks for a safe state, `critical` asks for it immediately, and a robot with
 * a dead gauge withholds leases while being in none of those rows.
 *
 * `unaccounted` is an **array in §36.2's own order**, not sorted: the terms are a sentence about
 * which parts of a computation happened, and canonical JSON would alphabetise an object into
 * nonsense.
 */
export function shapePower(
  profile: PowerProfile, observation: PowerObservation,
  declaredBudgetMs?: number, observedInferenceMs?: number,
  /** §5's base kind, when the document that carried the profile also carried one (§87.2). */
  base?: string | null,
): unknown {
  const assessment = assessPower(profile, observation, base)
  return {
    state: assessment.state,
    reason: assessment.reason,
    remaining_wh: nullable(assessment.remaining_wh),
    reserve: {
      wh: nullable(assessment.reserve.wh),
      unaccounted: [...assessment.reserve.unaccounted],
      complete: reserveComplete(assessment.reserve),
      // §87.2. Beside `unaccounted` rather than instead of it: the term leaving the list says the
      // computation happened, and the basis says *how*, and a reader who can see only the first is
      // reading a worst case over a warehouse as a measurement of where the robot is standing.
      safe_state_basis: nullable(assessment.reserve.safe_state_basis),
    },
    thermal: assessment.thermal,
    shed: [...assessment.shed],
    faults: assessment.faults.map(fault => ({ code: fault.code, detail: fault.detail })),
    degraded: powerDegraded(assessment),
    grants_new_leases: grantsNewLeases(assessment),
    requires_safe_state: stateRequiresSafeState(assessment.state),
    stops_immediately: stateStopsImmediately(assessment.state),
    // §36.1 derives three facts from a power state and this document carried two of them until
    // 2026-09-16. `stateNotifiesOperator` existed, was tested, and reached no surface in any
    // edition — so the row that says somebody is told was computed and told to nobody. A robot
    // that has stopped taking work and a robot that has stopped taking work *and owes an operator
    // a phone call* are different situations, and only the first was reportable.
    notifies_operator: stateNotifiesOperator(assessment.state),
    budget_missed: nullable(budgetMissed(declaredBudgetMs, observedInferenceMs)),
  }
}

/**
 * One assembled observation frame, as the black box records it (§25.1's last bullet).
 *
 * Payloads are absent by construction — this is the *index* of a frame, not the frame — which is
 * what makes it affordable to record every cycle. An incident investigation months later reads this
 * and can say which sample went in and how far apart they were, which is exactly what "the policy
 * behaved oddly" turns into once somebody has to check it.
 *
 * The stream rows are an **array in the order the streams were given**, which is the descriptor's
 * order. It is the only order in which two frames can be read row by row, and it is the same
 * reasoning §46.3's window list is an array: canonical JSON sorts an object's keys, and a table
 * whose rows reorder themselves between frames is a table nobody can diff.
 *
 * A stream with no sample gets five present-and-null columns rather than five absent keys, for the
 * duller half of the same reason: a console lays these out in a table, and a row with no columns
 * renders as blank rather than as "this stream had nothing".
 */
export function shapeObservation(frame: ObservationFrame): unknown {
  return {
    target_ns: frame.target_ns,
    streams: frame.streams.map(stream => ({
      name: stream.name,
      modality: stream.modality,
      latency_ms: nullable(stream.latency_ms),
      uncertainty_ms: nullable(stream.uncertainty_ms),
      budget_ms: nullable(stream.budget_ms),
      age_ms: stream.chosen === null ? null : stream.chosen.age_ms,
      considered: stream.chosen === null ? 0 : stream.chosen.considered,
      over_budget: stream.chosen === null ? false : stream.chosen.over_budget,
      seq: stream.chosen === null ? null : stream.chosen.seq,
      stamp_ns: stream.chosen === null ? null : stream.chosen.stamp_ns,
    })),
    skew_ms: nullable(frame.skew_ms),
    skew_upper_ms: nullable(frame.skew_upper_ms),
    undeclared_latency: [...frame.undeclared_latency],
    undeclared_budget: [...frame.undeclared_budget],
    holds: frame.holds.map(hold => ({
      stream: hold.stream, reason: hold.reason, detail: hold.detail,
    })),
    verdict: frame.verdict,
    deliverable: frame.deliverable,
    // The projection onto §26's record, inside the frame rather than beside it. It is derived from
    // everything above, and pinning it in a second corpus would let an edition agree about a frame
    // and disagree about the only version of it that survives the day.
    record: shapeObservationRecord(frame),
  }
}

function shapeObservationRecord(frame: ObservationFrame): unknown {
  const record = toRecord(frame)
  return {
    target_ns: record.target_ns,
    worst_skew_us: nullable(record.worst_skew_us),
    streams_absent: record.streams_absent,
    streams: record.streams.map(stream => ({
      name: stream.name, age_us: stream.age_us, stale: stream.stale,
    })),
  }
}

/**
 * §54.2's predicate answer: the boolean **and** the outcome, in one value.
 *
 * Pinned together deliberately. `evaluatePredicate` is defined as `satisfied(…)` of the outcome, so
 * an edition that computed the two separately would be free to disagree with itself — and the whole
 * argument for adding the third value is that it is a refinement of the boolean rather than a
 * second opinion. Freezing both in one case makes that checkable rather than asserted.
 */
export function shapePredicate(outcome: PredicateOutcome, boolean: boolean): unknown {
  return {
    satisfied: boolean,
    outcome: outcome.outcome,
    reason: nullable(outcome.reason),
    path: nullable(outcome.path),
    op: nullable(outcome.op),
  }
}

/** A parsed version, or null. */
export function shapeVersion(parsed: ParsedVersion | null): unknown {
  return parsed === null ? null : {
    major: parsed.major,
    minor: parsed.minor,
    patch: parsed.patch,
    prerelease: nullable(parsed.prerelease),
    raw: parsed.raw,
  }
}

/**
 * Range satisfaction, with **validity beside it**.
 *
 * `satisfies` answers false both for "outside the window" and for "that is not a window", and
 * §54.2 needs those apart. Pinning only the boolean would let an edition agree about every
 * satisfaction and disagree about which ranges it could read at all — which is the difference
 * between refusing a skill and telling its publisher why.
 */
export function shapeSatisfies(
  version: string, range: string,
  outcome: { satisfies: boolean; validVersion: boolean; validRange: boolean },
): unknown {
  return {
    version,
    range,
    satisfies: outcome.satisfies,
    valid_version: outcome.validVersion,
    valid_range: outcome.validRange,
  }
}

function shapeClauseResult(result: ClauseResult): unknown {
  return {
    index: result.index,
    statement: result.statement,
    origin: result.origin,
    hold: result.hold,
    outcome: result.outcome,
    reason: nullable(result.reason),
    path: nullable(result.path),
    op: nullable(result.op),
  }
}

/**
 * §54.2's report.
 *
 * `results` carries every clause even after one has refused, because an operator fixing four
 * problems one attempt at a time is the failure `admitPackage` already refuses to cause. `refused`
 * is the first, repeated rather than referenced by index, because it is what goes on a screen.
 */
export function shapePreconditions(report: PreconditionReport): unknown {
  return {
    phase: report.phase,
    permitted: report.permitted,
    refused: report.refused === null ? null : shapeClauseResult(report.refused),
    results: report.results.map(shapeClauseResult),
    unenforceable: report.unenforceable,
  }
}

/** §54.3's judgement. */
export function shapeSuccess(judgement: SuccessJudgement): unknown {
  return {
    outcome: judgement.outcome,
    deciding: judgement.deciding === null ? null : shapeClauseResult(judgement.deciding),
    results: judgement.results.map(shapeClauseResult),
  }
}

/** §54.4's timeout. `deadline_ms` is nullable rather than absent: "no deadline" is a fact. */
export function shapeDeadline(deadline: Deadline): unknown {
  return {
    deadline_ms: nullable(deadline.deadline_ms),
    enforceable: deadline.enforceable,
    factor: deadline.factor,
  }
}

/** §54.5's verdict, `{ fault: null }` when the graph is usable — the same shape as a frame fault. */
export function shapeComposition(fault: CompositionFault | null): unknown {
  return {
    fault: fault === null ? null : {
      code: fault.code,
      path: [...fault.path],
      detail: fault.detail,
    },
  }
}

/** §55.2's faults, all of them, in the order `validateCell` found them. */
export function shapeCellFaults(faults: CellFault[]): unknown {
  return {
    faults: faults.map(fault => ({
      code: fault.code,
      subjects: [...fault.subjects],
      detail: fault.detail,
    })),
  }
}

function shapeVolumeShape(shape: VolumeShape): unknown {
  return shape.kind === 'sphere'
    ? { kind: 'sphere', centre: [...shape.centre], radius_m: shape.radius_m }
    : { kind: 'box', min: [...shape.min], max: [...shape.max] }
}

/**
 * A cell's contribution to the `workspace` guard, `{ layer: null }` when it draws nothing.
 *
 * Nullable rather than absent, and the distinction is §55.2's: a cell nobody has measured and a
 * cell that permits nothing are different facts, and only one of them should stop a robot.
 */
export function shapeCellLayer(layer: WorkspaceLayer | null): unknown {
  return {
    layer: layer === null ? null : {
      name: layer.name,
      volumes: layer.volumes.map(volume => ({
        frame: volume.frame,
        shape: shapeVolumeShape(volume.shape),
      })),
    },
  }
}

/**
 * A cell's robots, as §87.1 reports them: the derived facts, the per-robot layers, and the faults.
 *
 * One document rather than three, for `shapeWatchdog`'s reason: the three are one question — *what
 * does this cell say about its robots* — and an edition could agree about the disjointness
 * arithmetic while disagreeing about which layer a robot contributes, which is the half a workspace
 * guard reads. The layers are an **array in the cell's own declaration order**, because that is the
 * order an operator wrote them in and `cellRobotSlugs` is where the sorted view lives.
 */
export function shapeCellRobots(cell: CellDescriptor, subject: string | null): unknown {
  return {
    env: cellEnvironment(cell, subject),
    slugs: cellRobotSlugs(cell),
    layers: (cell.robots ?? []).map(robot => ({
      slug: typeof robot.slug === 'string' ? robot.slug : '',
      ...shapeCellLayer(
        cellRobotLayer(cell, typeof robot.slug === 'string' ? robot.slug : ''),
      ) as Record<string, unknown>,
    })),
    faults: (validateCell(cell)).map(fault => ({
      code: fault.code,
      subjects: [...fault.subjects],
      detail: fault.detail,
    })),
  }
}

/**
 * §87.2's safe-state declaration: the fault, the basis the reserve may account on, and the tier.
 *
 * The three together rather than the fault alone, because §87.2's whole argument is that a figure
 * and its provenance are one fact: a document carrying `basis_undeclared` and not carrying *"and
 * therefore host is refused and the reserve term is unaccounted"* would leave a reader to re-derive
 * the consequence, which is where the two editions would disagree.
 */
export function shapeSafeState(profile: PowerProfile, base: string | null): unknown {
  const fault = validateSafeState(profile, base)
  return {
    base: nullable(base),
    fault: fault === null ? null : { code: fault.code, detail: fault.detail },
    basis: nullable(safeStateBasis(profile, base)),
    host_blocked: nullable(hostBlockedBySafeState(profile, base)),
    reserve: (() => {
      const budget = powerReserve(profile, base)
      return {
        wh: nullable(budget.wh),
        unaccounted: [...budget.unaccounted],
        safe_state_basis: nullable(budget.safe_state_basis),
      }
    })(),
  }
}

/**
 * §87.4's relay: the session's own verdict, and the whole permission table beside it.
 *
 * The table rather than one answer, for §78's reason: a surface that asked per control would be a
 * second place §39.1's rules are written down, in whatever order its layout happened to be. In
 * `CONSOLE_ACTIONS` order, which is §39.1's table order with `stop_request` first, because a console
 * that drew the stop last would be a console somebody had to scroll.
 */
export function shapeRelay(session: ConsoleSession, nowMs: number): unknown {
  const verdict = relaySession(session, nowMs)
  return {
    session: {
      route: session.route,
      role: session.role,
      state: verdict.state,
      refusal: nullable(verdict.refusal),
      detail: verdict.detail,
      expires_at_ms: nullable(verdict.expires_at_ms),
    },
    actions: relayPermissions(session, nowMs).map(action => ({
      action: action.action,
      allowed: action.allowed,
      refusal: nullable(action.refusal),
      bounded_by_session: action.bounded_by_session,
      detail: action.detail,
    })),
  }
}

/**
 * §88's verdict over one motion request.
 *
 * `arguments` is an **array in the motion's declaration order**, which is the whole reason it is on
 * the verdict rather than left to the caller: a program-level controller's motion is a call with
 * positional arguments, and an object would be alphabetised by canonical JSON into an argument list
 * no controller agreed to.
 */
export function shapeMotionVerdict(gate: MotionGate, request: MotionRequest): unknown {
  const verdict = admitMotion(gate, request)
  return {
    accepted: verdict.accepted,
    refusal: nullable(verdict.refusal),
    detail: verdict.detail,
    arguments: verdict.arguments.map(entry => ({ name: entry.name, value: entry.value })),
    unbounded: [...verdict.unbounded],
  }
}

/** §88.3's per-cycle question about a motion already in flight. */
export function shapeMotionProgress(
  deadlineMs: number, startedAtMs: number, nowMs: number,
): unknown {
  const report = motionProgress(deadlineMs, startedAtMs, nowMs)
  return {
    progress: report.progress,
    overdue_by_ms: report.overdue_by_ms,
    detail: report.detail,
  }
}

/**
 * §54.2's live context, with the two omission lists beside it rather than inside it.
 *
 * Beside, because a path under `robot.` describing the context itself would be a path a
 * precondition could read — and a skill whose precondition asserts something about the completeness
 * of its own evaluation context is a skill asserting something about the OS rather than about the
 * robot.
 */
export function shapeLiveContext(built: LiveContext): unknown {
  return {
    context: built.context,
    unaddressable: [...built.unaddressable],
    unreadable: [...built.unreadable],
  }
}

/**
 * §12's verdict on one OS bundle.
 *
 * `subjects` is always present and may be empty, which is the opposite of the rule most findings in
 * this file follow — and the reason is that a *reader* of this list groups by subject. A finding
 * whose subject key came and went would make that grouping a special case per code.
 */
export function shapeBundleVerdict(verdict: BundleVerdict): unknown {
  return {
    admitted: verdict.admitted,
    faults: verdict.faults.map(fault => ({
      code: fault.code,
      severity: fault.severity,
      message: fault.message,
      subjects: [...fault.subjects],
    })),
    gated_components: [...verdict.gated_components],
  }
}

/** §12's decision about a slot the robot has just booted into. */
export function shapeBootVerdict(verdict: BootVerdict): unknown {
  return {
    decision: verdict.decision,
    detail: verdict.detail,
    missing: [...verdict.missing],
  }
}

/** §27's machine, as one evaluation. Every field is a primitive; there is nothing nullable here. */
export function shapeTransition(transition: Transition): unknown {
  return {
    from: transition.from,
    to: transition.to,
    reason: transition.reason,
    changed: transition.changed,
    warning: transition.warning,
  }
}

/**
 * The three properties other sections read off a state, for every state.
 *
 * Pinned beside the transitions because §33.4's barrier, §49.1's tone and §27's stickiness are read
 * off the *state* rather than off the transition — an edition could agree about every evaluation
 * while disagreeing about what a state means, and no evaluation case would see it.
 */
export function shapeStateProperties(): unknown {
  return OPERATING_STATES.map(state => ({
    state,
    safe_state: isSafeState(state),
    commands: commandsAutonomously(state),
    sticky: isSticky(state),
  }))
}

/**
 * Every ordered pair of states and whether it sounds §49.1's tone.
 *
 * Forty-nine rows, which is small enough to freeze in full — and the only way to be sure two
 * editions agree about a predicate whose failure mode is silence.
 */
export function shapeWarningMatrix(): unknown {
  return OPERATING_STATES.flatMap(
    from => OPERATING_STATES.map(to => ({ from, to, warning: warningRequired(from, to) })),
  )
}

/**
 * §54.4's lifecycle, as one evaluation.
 *
 * `outcome` is **nullable rather than absent**, which is this file's rule applied to the field it
 * matters most for: "this transition is not terminal" is a fact, and a consumer that had to
 * distinguish a missing key from a null would get it wrong exactly once.
 */
export function shapeSkillTransition(transition: SkillTransition): unknown {
  return {
    from: transition.from,
    to: transition.to,
    reason: transition.reason,
    outcome: nullable(transition.outcome),
    detail: transition.detail,
    release_leases: transition.release_leases,
    changed: transition.changed,
  }
}

/**
 * What a robot made of the characters somebody typed into `skynet pair`.
 *
 * `code` and `fault` are both present and both nullable, and a caller reads exactly one of them —
 * so a shape where the other key came and went would make "which of the two happened" a question
 * about key presence rather than about a value, and the corpus would be pinning a shape rather
 * than a decision.
 *
 * The nesting is not cosmetic. `{ fault: null | { code, detail, … } }` is the shape frame
 * validation and the cell descriptor already emit, which is what puts these refusals inside the
 * §64 check rather than beside it.
 */
export function shapeCodeResult(result: CodeResult): unknown {
  const fault = result.fault ?? null
  return {
    code: nullable(result.code),
    fault: fault === null ? null : {
      code: fault.code,
      detail: fault.detail,
      position: nullable(fault.position),
    },
  }
}

/** §10's answer about one robot's identity. */
export function shapeIdentityVerdict(verdict: IdentityVerdict): unknown {
  return {
    attested: verdict.attested,
    findings: verdict.findings.map(entry => ({
      code: entry.code,
      message: entry.message,
      severity: entry.severity,
      subjects: [...entry.subjects],
    })),
    paired: verdict.paired,
  }
}

/**
 * §50's answer about one certificate and one machine.
 *
 * `attests`, `fully_confirmed` and `unchecked` are derived rather than stored, and all three are
 * on the wire anyway. A consumer recomputing "does this attest anything" from a finding list is a
 * consumer with its own copy of `voidsTheCertificate`, and the second copy is the one that will be
 * wrong the day a finding is added.
 */
export function shapeAttested(result: BirthAttested): unknown {
  return {
    attests: birthAttests(result),
    findings: result.findings.map(finding => ({
      code: finding.code,
      message: birthExplain(finding),
      subjects: [...finding.subjects],
    })),
    fully_confirmed: birthFullyConfirmed(result),
    unchecked: birthUnchecked(result),
  }
}

/**
 * §56.2's class for one run bundle.
 *
 * `rankable`, `ranked_separately` and `replayable` are derived and all three go on the wire: a
 * leaderboard recomputing "may this be ranked" from a class string is a leaderboard with its own
 * copy of §56.2, and the second copy is the one that will admit a simulator to the field table the
 * day a class is added.
 */
export function shapeProvenance(classified: Classified): unknown {
  return {
    class: classified.class,
    rankable: rankable(classified.class),
    ranked_separately: rankedSeparately(classified.class),
    reasons: classified.reasons.map(reason => ({
      blocks_field: blocksField(reason),
      code: reason.code,
      message: provenanceExplain(reason),
      subjects: [...reason.subjects],
    })),
    replayable: replayable(classified),
  }
}

/**
 * §56.5's roll-up comparison.
 *
 * The sentence travels beside the boolean rather than being left to the caller, because §56.5's
 * whole point is that a disagreement is a defect to investigate — and a bare `false` is the shape
 * of a thing somebody quietly widens a tolerance around.
 */
export function shapeScores(onRobot: number, onHub: number): unknown {
  const agree = scoresAgree(onRobot, onHub)
  return {
    agree,
    detail: agree ? null : explainDisagreement(onRobot, onHub),
  }
}

/** §10's sentence, and what is missing from it. */
export function shapeAttestationReport(report: AttestationReport): unknown {
  return {
    complete: report.complete,
    sentence: report.sentence,
    unmeasured: [...report.unmeasured],
  }
}

/**
 * One measure, with the digest kept rather than rendered away.
 *
 * `digest` and `why` are present-and-null rather than omitted, for the reason
 * `shapeHealthReport` keeps all five of §33.6's fields: a consumer that has to discover which keys
 * this robot happened to fill in is a consumer that will get it wrong on the robot that filled in
 * none.
 */
function shapeMeasure(measure: Measure): unknown {
  return {
    digest: measure.state === 'known' ? nullable(trimmedOrNull(measure.digest)) : null,
    state: measure.state,
    why: nullable(trimmedOrNull(measure.why)),
  }
}

function trimmedOrNull(value: string | null | undefined): string | null {
  const text = (value ?? '').trim()
  return text === '' ? null : text
}

/**
 * The attestation as §10 asks for it: the sentence **and** the three digests it is made of.
 *
 * # Why the sentence alone was not enough
 *
 * §10 asks the Hub to record *"robot X is running image `sha256:…`, policy `sha256:…`, against body
 * `sha256:…`"*, and calls it the sentence an incident investigation needs and nobody can currently
 * produce. `attest` produces it, and for two sessions it was the whole of what a check-in carried.
 *
 * A sentence is not a record. Ask the fleet *"which robots are running image `sha256:aa`"* — the
 * question §32.1 puts a column on — and the only way to answer it from a sentence is to parse
 * English back out of prose that deliberately is not machine-readable: `no policy`, `an unmeasured
 * OS image (the slot could not be read)`, `an unnamed body`. Those renderings are right for a person
 * and useless for an index, and a Hub that regex'd them would be re-deriving a fact the robot had
 * and threw away.
 *
 * So both go on the wire. The sentence, because it is what somebody reads at 3 a.m.; the measures,
 * because they are what somebody queries at 3 a.m. — and because a digest that survives only inside
 * a rendering is a digest that a change to the rendering silently deletes.
 */
export function shapeAttestation(attestation: Attestation): unknown {
  return {
    ...(shapeAttestationReport(attestOfMeasures(attestation)) as Record<string, unknown>),
    embodiment: shapeMeasure(attestation.embodiment),
    os_image: shapeMeasure(attestation.os_image),
    policy: shapeMeasure(attestation.policy),
  }
}

/** What a robot may claim about the generation it was given (§17, §33.3). */
export function shapeClaimVerdict(verdict: ClaimVerdict): unknown {
  return {
    applied: verdict.applied,
    blocking: [...verdict.blocking],
    claim: verdict.claim,
    detail: verdict.detail,
    generation: verdict.generation,
  }
}

/**
 * §33.6's health, rolled up.
 *
 * `fields` carries **every** name in `HEALTH_FIELDS`, present and null where there is no dated
 * reading. That is this file's nullable rule, and it is load-bearing rather than tidy: a console
 * iterating the five names §33.6 lists must not have to discover which of them a particular robot
 * happened to measure, and a key that came and went would make "unmeasured" a question about
 * key presence.
 */
export function shapeHealthReport(report: HealthReport): unknown {
  const fields: Record<string, unknown> = {}
  for (const name of HEALTH_FIELDS) {
    const held = report.fields[name] ?? null
    fields[name] = held === null ? null : { age_ms: held.age_ms, value: held.value }
  }
  return {
    degraded: report.degraded,
    fields,
    packages: report.packages.map(entry => ({
      age_ms: entry.age_ms,
      id: entry.id,
      live: entry.live,
      readiness: entry.readiness,
    })),
    stalled: [...report.stalled],
    undated: [...report.undated],
    unmeasured: [...report.unmeasured],
  }
}

/**
 * The whole check-in, as it goes on the wire.
 *
 * Frozen as one document rather than as its parts, because this is the artifact §17 describes and
 * the one two editions must not differ over. A corpus that pinned the claim and the health
 * separately would let an edition agree about both and still send different bytes.
 */
export function shapeCheckIn(built: CheckIn): unknown {
  return {
    agent: built.agent,
    applied: built.claim.applied,
    attestation: shapeAttestation(built.attestation),
    blocking: [...built.claim.blocking],
    claim: built.claim.claim,
    contracts: built.contracts,
    desired_digest: built.desired_digest,
    detail: built.claim.detail,
    device_id: built.device_id,
    generation: built.claim.generation,
    health: shapeHealthReport(built.health),
    // §7.6's ceilings and guard partition, or null. Present-and-null so a consumer can tell "this
    // edition does not report them" from "this robot resolved none".
    limits: shapeGuardReport(built.limits),
    // §23.5's machine readings, or null. Present-and-null for the same reason: an edition that
    // does not read the machine is not a machine that backs no claim.
    machine: built.machine === null
      ? null
      : {
          preempt_rt: built.machine.preempt_rt,
          swap_kb: built.machine.swap_kb,
        },
    pending: built.pending.map(entry => ({
      deferred: entry.deferred,
      reason: entry.reason,
      stage: entry.stage,
    })),
    state: built.state,
  }
}

/** §7.6's guard report, as a check-in carries it. */
export function shapeGuardReport(report: GuardReport | null): unknown {
  if (report === null) return null
  return {
    active: [...report.active],
    base_velocity_max_m_s: report.base_velocity_max_m_s,
    enforced: report.enforced,
    enforcement: report.enforcement,
    joint_torque_max_nm: report.joint_torque_max_nm,
    joint_velocity_max_rad_s: report.joint_velocity_max_rad_s,
    max_delta_per_cycle: report.max_delta_per_cycle,
    self_collision_margin_m: report.self_collision_margin_m,
    min_control_hz: report.min_control_hz,
    notices: report.notices.map(notice => ({
      code: notice.code,
      subjects: [...notice.subjects],
    })),
    resolved_at: report.resolved_at,
    source: report.source,
    unavailable: report.unavailable.map(entry => ({
      guard: entry.guard,
      missing: [...entry.missing],
    })),
    // §80's third state. Always present, empty included: a field a robot omits where it is empty
    // would let an edition that never emits it pass this frozen document (§31.6).
    inert: report.inert.map(entry => ({
      guard: entry.guard,
      reading: entry.reading,
    })),
  }
}

/**
 * §74's answer for one telemetry window.
 *
 * `rates` is an object rather than a list, and the counters that were *not* measured are absent from
 * it rather than present as zero — which is the whole of §74.3 in one shape. A consumer iterating
 * `TELEMETRY_COUNTERS` and reading `rates[name] ?? 0` would be committing the mistake the section
 * exists to prevent; one that reads `unmeasured` first cannot.
 */
export function shapeWindowReport(report: WindowReport): unknown {
  const rates: Record<string, number> = {}
  for (const [name, rate] of Object.entries(report.rates)) rates[name] = rate
  return {
    elapsed_ms: report.elapsed_ms,
    faults: report.faults.map(entry => ({
      code: entry.code,
      message: entry.message,
      severity: entry.severity,
    })),
    rates,
    skew_ms: report.skew_ms,
    unmeasured: [...report.unmeasured],
    usable: report.usable,
  }
}

/**
 * What a set of windows covers (§74.1).
 *
 * `gaps` is the field this shape exists for. §47.4's rule — a gap annotated as a gap is honest, a
 * gap that looks like a quiet period is not — is obtained here from the data rather than from an
 * annotation, and a shape that dropped the gaps would leave a fleet page unable to tell a robot
 * that reported nothing from one whose telemetry was discarded under §34.4's drop policy.
 */
export function shapeCoverage(value: Coverage): unknown {
  const totals: Record<string, number> = {}
  for (const [name, total] of Object.entries(value.totals)) totals[name] = total
  return {
    // §15's third suite, over what was actually observed. Nullable rather than optional, and
    // frozen here rather than left to each consumer, because this is the number that decides
    // whether a certificate is suspended — and it had two implementations, in two crates, pinned
    // by nothing. A value both editions compute and neither corpus holds is a value they are free
    // to disagree about.
    conformance: nullable(conformanceOf(value)),
    gaps: value.gaps.map(gap => ({ from_ms: gap.from_ms, to_ms: gap.to_ms })),
    observed_ms: value.observed_ms,
    spanned_ms: value.spanned_ms,
    totals,
    unmeasured: [...value.unmeasured],
    unusable: value.unusable,
  }
}

/**
 * §11's answer for one offered desired state.
 *
 * `changed` sits beside `adopt` rather than being derived from it by a reader, because the two
 * differ on the most common case there is: an ordinary poll adopts a document identical to the one
 * on disk, and a caller that inferred "adopted, therefore write it" would rewrite the desired state
 * every few seconds.
 */
export function shapeOfferVerdict(verdict: OfferVerdict): unknown {
  return {
    adopt: verdict.adopt,
    changed: verdict.changed,
    faults: verdict.faults.map(entry => ({
      code: entry.code,
      message: entry.message,
      severity: entry.severity,
    })),
    generation: verdict.generation,
  }
}
