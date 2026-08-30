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
