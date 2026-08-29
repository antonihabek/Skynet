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
import type { CycleVerdict, ResolvedLimits } from './safety.ts'

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
