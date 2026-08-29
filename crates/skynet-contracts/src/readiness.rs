//! OS readiness — what the robot registry actually supports claiming today.
//!
//! [`crate::certification`] awards a tier from *measurements*: an estop latency taken at the
//! actuator, a joint that refused 150% of its rating, a rollback that recovered unattended. None of
//! those exist until somebody runs a bench suite against real hardware.
//!
//! This module answers the question that can be answered now, from the registry alone: **given what
//! has been declared about this robot, which tier could it even attempt?** A robot with no declared
//! safety envelope cannot attempt the host tier — not because it failed a test, but because there
//! is nothing for the Safety Kernel to enforce.
//!
//! # Why this is a separate concept from certification
//!
//! Collapsing the two would produce the exact lie the platform is built to avoid: a page reading
//! "host tier" over a robot nobody has ever measured. So the vocabulary is deliberately different.
//! This module returns an **attainable** tier and a list of **blockers**;
//! [`crate::certification::award_certification`] returns an *awarded* tier. Attainable means "the
//! paperwork is in order and you may now go and measure it". It is never evidence of anything about
//! the machine.
//!
//! The distinction has a pleasant side effect: the blockers are a to-do list. A vendor looking at
//! "declares no watchdog window" knows precisely what to publish next, which is more useful than a
//! red cross.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::embodiment::SafetyEnvelope;

/// The subset of a registry row readiness depends on.
///
/// Structural rather than the full robot record, so the checks can be exercised directly and so a
/// caller assembling a robot from somewhere other than the registry is not forced to fabricate
/// twenty irrelevant columns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadinessSubject {
    /// Registry slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Degrees of freedom, when declared.
    pub dof: Option<f64>,
    /// Capability tokens declared.
    pub capabilities: Vec<String>,
    /// Control modes declared.
    pub control_modes: Vec<String>,
    /// Control rate declared.
    pub control_hz: Option<f64>,
    /// Whether any onboard compute was described. Empty means nothing was.
    pub compute_keys: Vec<String>,
    /// `None` means nobody declared an envelope. Distinct from `Some(default)`, which means one
    /// was declared and is empty.
    pub limits: Option<SafetyEnvelope>,
    /// How many published adapters exist. Zero means nothing can talk to this robot yet.
    pub adapter_count: f64,
}

/// The checks, in the order they are reported.
pub const READINESS_CHECKS: [&str; 11] = [
    "adapter",
    "kinematics",
    "control_modes",
    "control_rate",
    "envelope",
    "envelope_source",
    "watchdog",
    "joint_ratings",
    "estop_software",
    "estop_hardware",
    "onboard_compute",
];

/// One check's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    /// Which check.
    pub check: &'static str,
    /// Whether the declaration is present and usable.
    pub ok: bool,
    /// Lowest tier that needs this. A failed `guest` check blocks everything above it too.
    pub required_for: &'static str,
    /// English, for logs and API clients. A UI translates by `check`.
    pub detail: String,
}

/// One robot's readiness.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadinessReport {
    /// Registry slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Highest tier this robot could *attempt*, given what has been declared.
    ///
    /// Never a claim about the machine. `None` means it cannot yet attempt even the guest tier,
    /// which in practice means no adapter exists.
    pub attainable: Option<String>,
    /// Every check.
    pub checks: Vec<CheckResult>,
    /// Failed checks, in tier order — the vendor's to-do list.
    pub blockers: Vec<CheckResult>,
    /// Share of checks passing, for sorting a fleet-wide view. Not a score anyone should quote.
    pub completeness: f64,
}

const TIER_ORDER: [&str; 3] = ["guest", "host", "certified"];

/// Evaluate one robot.
///
/// Pure. Every check is a question about a declared value, and a check whose input is absent
/// **fails rather than passing** — the same rule [`crate::certification::award_certification`]
/// applies to a missing test result, for the same reason. Absence is not evidence of compliance.
pub fn evaluate_readiness(subject: &ReadinessSubject) -> ReadinessReport {
    let has_cap = |token: &str| subject.capabilities.iter().any(|c| c == token);
    let limits = subject.limits.as_ref();
    let n = crate::json::number_text;

    let mut checks: Vec<CheckResult> = Vec::with_capacity(READINESS_CHECKS.len());

    checks.push(CheckResult {
        check: "adapter",
        ok: subject.adapter_count > 0.0,
        required_for: "guest",
        detail: if subject.adapter_count > 0.0 {
            format!(
                "{} published adapter{}.",
                n(subject.adapter_count),
                if subject.adapter_count == 1.0 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            "No published adapter, so nothing can talk to this robot yet.".to_string()
        },
    });

    let dof_ok = subject.dof.is_some_and(|d| d > 0.0);
    checks.push(CheckResult {
        check: "kinematics",
        ok: dof_ok,
        required_for: "guest",
        detail: match subject.dof.filter(|d| *d != 0.0) {
            Some(dof) => format!("{} degrees of freedom declared.", n(dof)),
            None => "No degree-of-freedom count declared.".to_string(),
        },
    });

    checks.push(CheckResult {
        check: "control_modes",
        ok: !subject.control_modes.is_empty(),
        required_for: "guest",
        detail: if subject.control_modes.is_empty() {
            "No control modes declared, so no policy can be matched to it.".to_string()
        } else {
            format!("Accepts {}.", subject.control_modes.join(", "))
        },
    });

    checks.push(CheckResult {
        check: "control_rate",
        ok: subject.control_hz.is_some_and(|hz| hz > 0.0),
        required_for: "host",
        detail: match subject.control_hz.filter(|hz| *hz != 0.0) {
            Some(hz) => format!("Control loop runs at {} Hz.", n(hz)),
            None => {
                "No control rate declared, so loop stability cannot be checked against a policy."
                    .to_string()
            }
        },
    });

    // Null and an empty envelope are deliberately different: nobody looked, versus somebody looked
    // and found nothing. Only the first blocks, and the message says which.
    checks.push(CheckResult {
        check: "envelope",
        ok: limits.is_some(),
        required_for: "host",
        detail: if limits.is_none() {
            "No safety envelope declared. There is nothing for the kernel to enforce.".to_string()
        } else {
            "A safety envelope has been declared.".to_string()
        },
    });

    let source = limits
        .and_then(|l| l.source.as_deref())
        .filter(|s| !s.is_empty());
    checks.push(CheckResult {
        check: "envelope_source",
        ok: source.is_some(),
        required_for: "host",
        detail: match source {
            Some(source) => format!("Limits cite {source}."),
            None => "Limits cite no datasheet. A safety limit without provenance is a rumour."
                .to_string(),
        },
    });

    let watchdog = limits.and_then(|l| l.watchdog_ms).filter(|ms| *ms > 0.0);
    checks.push(CheckResult {
        check: "watchdog",
        ok: watchdog.is_some(),
        required_for: "host",
        detail: match watchdog {
            Some(ms) => format!("Controller faults after {} ms without a command.", n(ms)),
            None => "No watchdog window declared, so the watchdog guard cannot run.".to_string(),
        },
    });

    let velocity = limits.and_then(|l| l.joint_velocity_max_rad_s);
    let torque = limits.and_then(|l| l.joint_torque_max_nm);
    checks.push(CheckResult {
        check: "joint_ratings",
        ok: velocity.is_some() && torque.is_some(),
        required_for: "host",
        detail: match (velocity.filter(|v| *v != 0.0), torque.filter(|t| *t != 0.0)) {
            (Some(velocity), Some(torque)) => {
                format!("Rated to {} rad/s and {} Nm.", n(velocity), n(torque))
            }
            _ => {
                "Velocity and torque ceilings are not both declared, so commands cannot be clamped."
                    .to_string()
            }
        },
    });

    checks.push(CheckResult {
        check: "estop_software",
        ok: has_cap("safety.estop"),
        required_for: "guest",
        detail: if has_cap("safety.estop") {
            "Exposes a software emergency stop.".to_string()
        } else {
            "No software emergency stop capability declared.".to_string()
        },
    });

    let hardware_estop = limits.and_then(|l| l.estop) == Some(true);
    checks.push(CheckResult {
        check: "estop_hardware",
        ok: hardware_estop,
        required_for: "certified",
        detail: if hardware_estop {
            "A hardware emergency stop is fitted.".to_string()
        } else {
            "No hardware emergency stop declared. The certified tier needs a stop path off the application processor.".to_string()
        },
    });

    let compute = has_cap("platform.onboard_compute") || !subject.compute_keys.is_empty();
    checks.push(CheckResult {
        check: "onboard_compute",
        ok: compute,
        required_for: "host",
        detail: if compute {
            "Runs inference onboard.".to_string()
        } else {
            "No onboard compute declared, so the OS cannot own the control loop here.".to_string()
        },
    });

    let attainable = highest_attainable(&checks);

    let mut blockers: Vec<CheckResult> = checks.iter().filter(|c| !c.ok).cloned().collect();
    // Stable sort by tier, so the to-do list reads from "cannot do anything" downward and two
    // blockers at the same tier keep the order the checks were declared in.
    blockers.sort_by_key(|c| tier_rank(c.required_for));

    let passing = checks.iter().filter(|c| c.ok).count();
    let completeness = passing as f64 / checks.len() as f64;

    ReadinessReport {
        slug: subject.slug.clone(),
        name: subject.name.clone(),
        attainable,
        checks,
        blockers,
        completeness,
    }
}

fn tier_rank(tier: &str) -> usize {
    TIER_ORDER
        .iter()
        .position(|t| *t == tier)
        .unwrap_or(usize::MAX)
}

/// Walk upward and stop at the first tier with an unmet requirement.
///
/// Stopping rather than skipping matters: a robot that satisfies every certified-tier check but
/// lacks a watchdog window is not "certified with a caveat", it is a robot that cannot attempt the
/// host tier. Tiers are cumulative and the walk enforces that.
fn highest_attainable(checks: &[CheckResult]) -> Option<String> {
    let mut best: Option<String> = None;
    for tier in TIER_ORDER {
        let unmet = checks
            .iter()
            .any(|check| check.required_for == tier && !check.ok);
        if unmet {
            return best;
        }
        best = Some(tier.to_string());
    }
    best
}

/// Fleet-wide totals.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetReadiness {
    /// How many robots were evaluated.
    pub total: usize,
    /// How many robots could attempt each tier, counted at their highest tier only.
    /// Keys are `none`, `guest`, `host`, `certified`, always all four and always in that order.
    pub by_tier: Vec<(&'static str, usize)>,
    /// The checks failing most often across the registry — where the ecosystem is actually stuck.
    pub common_blockers: Vec<(&'static str, usize)>,
}

/// Aggregate across the registry.
///
/// `common_blockers` is the number worth putting on a screen: it says whether the ecosystem's
/// problem is missing adapters or missing datasheets, and those imply completely different work.
pub fn summarize_readiness(reports: &[ReadinessReport]) -> FleetReadiness {
    let mut by_tier: Vec<(&'static str, usize)> =
        alloc::vec![("none", 0), ("guest", 0), ("host", 0), ("certified", 0),];
    // Counted in **first-encounter order** — the order a check first appears as somebody's
    // blocker, walking the reports and then each report's blockers. That is what the reference
    // implementation's insertion-ordered map produces, and since the sort below is stable it is
    // the tie-break two editions have to share. Declaration order would arguably be a nicer
    // property; it would also be a different answer, and a corpus case with two equally common
    // blockers would then diverge.
    let mut counts: Vec<(&'static str, usize)> = Vec::new();

    for report in reports {
        let key = report.attainable.as_deref().unwrap_or("none");
        if let Some(entry) = by_tier.iter_mut().find(|(tier, _)| *tier == key) {
            entry.1 += 1;
        }
        for blocker in &report.blockers {
            match counts.iter_mut().find(|(check, _)| *check == blocker.check) {
                Some(entry) => entry.1 += 1,
                None => counts.push((blocker.check, 1)),
            }
        }
    }

    let mut common_blockers = counts;
    common_blockers.sort_by_key(|entry| core::cmp::Reverse(entry.1));

    FleetReadiness {
        total: reports.len(),
        by_tier,
        common_blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> ReadinessSubject {
        ReadinessSubject {
            slug: "unitree-g1".to_string(),
            name: "Unitree G1".to_string(),
            dof: Some(23.0),
            capabilities: alloc::vec![
                "safety.estop".to_string(),
                "platform.onboard_compute".to_string()
            ],
            control_modes: alloc::vec!["position".to_string(), "torque".to_string()],
            control_hz: Some(500.0),
            compute_keys: alloc::vec!["gpu".to_string()],
            limits: Some(SafetyEnvelope {
                joint_velocity_max_rad_s: Some(12.0),
                joint_torque_max_nm: Some(120.0),
                base_velocity_max_m_s: Some(2.0),
                watchdog_ms: Some(50.0),
                estop: Some(true),
                source: Some("https://example.invalid/g1.pdf".to_string()),
            }),
            adapter_count: 2.0,
        }
    }

    #[test]
    fn a_fully_declared_robot_can_attempt_the_certified_tier() {
        let report = evaluate_readiness(&complete());
        assert_eq!(report.attainable.as_deref(), Some("certified"));
        assert!(report.blockers.is_empty());
        assert_eq!(report.completeness, 1.0);
    }

    #[test]
    fn no_adapter_means_nothing_can_be_attempted() {
        let subject = ReadinessSubject {
            adapter_count: 0.0,
            ..complete()
        };
        let report = evaluate_readiness(&subject);
        assert_eq!(report.attainable, None);
    }

    #[test]
    fn a_missing_watchdog_stops_the_walk_at_guest() {
        let mut subject = complete();
        subject.limits.as_mut().unwrap().watchdog_ms = None;
        let report = evaluate_readiness(&subject);
        assert_eq!(
            report.attainable.as_deref(),
            Some("guest"),
            "not 'certified with a caveat'"
        );
    }

    #[test]
    fn a_declared_but_empty_envelope_is_not_a_missing_one() {
        let mut subject = complete();
        subject.limits = Some(SafetyEnvelope::default());
        let report = evaluate_readiness(&subject);
        let envelope = report
            .checks
            .iter()
            .find(|c| c.check == "envelope")
            .unwrap();
        assert!(envelope.ok, "somebody looked");
        let source = report
            .checks
            .iter()
            .find(|c| c.check == "envelope_source")
            .unwrap();
        assert!(!source.ok, "and found nothing");
    }

    #[test]
    fn blockers_are_a_to_do_list_in_tier_order() {
        let subject = ReadinessSubject {
            adapter_count: 0.0,
            control_hz: None,
            limits: None,
            capabilities: Vec::new(),
            ..complete()
        };
        let report = evaluate_readiness(&subject);
        let tiers: Vec<&str> = report.blockers.iter().map(|b| b.required_for).collect();
        let mut sorted = tiers.clone();
        sorted.sort_by_key(|t| tier_rank(t));
        assert_eq!(tiers, sorted);
        assert_eq!(report.blockers[0].required_for, "guest");
    }

    #[test]
    fn a_blocker_names_what_to_publish_next() {
        let mut subject = complete();
        subject.limits.as_mut().unwrap().source = None;
        let report = evaluate_readiness(&subject);
        let blocker = report
            .blockers
            .iter()
            .find(|b| b.check == "envelope_source")
            .unwrap();
        assert!(blocker.detail.contains("provenance"));
    }

    #[test]
    fn the_fleet_view_says_where_the_ecosystem_is_stuck() {
        let no_adapter = evaluate_readiness(&ReadinessSubject {
            adapter_count: 0.0,
            ..complete()
        });
        let no_envelope = evaluate_readiness(&ReadinessSubject {
            limits: None,
            ..complete()
        });
        let fine = evaluate_readiness(&complete());

        let fleet = summarize_readiness(&[no_adapter, no_envelope, fine]);
        assert_eq!(fleet.total, 3);
        assert_eq!(
            fleet.by_tier,
            alloc::vec![("none", 1), ("guest", 1), ("host", 0), ("certified", 1)]
        );
        assert!(fleet
            .common_blockers
            .iter()
            .any(|(check, n)| *check == "adapter" && *n == 1));
        assert!(fleet
            .common_blockers
            .iter()
            .any(|(check, n)| *check == "envelope" && *n == 1));
    }

    #[test]
    fn every_check_appears_in_the_report() {
        let report = evaluate_readiness(&complete());
        assert_eq!(report.checks.len(), READINESS_CHECKS.len());
        for name in READINESS_CHECKS {
            assert!(report.checks.iter().any(|c| c.check == name), "{name}");
        }
    }
}
