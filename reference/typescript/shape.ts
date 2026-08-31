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

import type { FrameFault, TransportPlan } from './embodiment.ts'
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
  CHAIN, WINDOWS, checkNesting, hostBlockedByCommandLoss, type Windows,
} from './watchdog.ts'
import { toRecord, type Frame as ObservationFrame } from './observation.ts'
import type { PredicateOutcome } from './predicate.ts'
import type { ParsedVersion } from './semver.ts'
import type {
  ClauseResult, CompositionFault, Deadline, PreconditionReport, SuccessJudgement,
} from './skill.ts'
import type { CellFault } from './cell.ts'
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
    per_channel: limits.per_channel,
    active_guards: limits.active_guards,
    unavailable_guards: limits.unavailable_guards.map(entry => ({
      guard: entry.guard,
      missing: entry.missing,
    })),
    source: nullable(limits.source),
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
export function shapeWatchdog(onCommandLoss: string | null, windows: Windows): unknown {
  const report = checkNesting(windows)
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
    host_blocked: nullable(hostBlockedByCommandLoss(onCommandLoss)),
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
