//! Calibration validity (§45).
//!
//! Calibration is the only state in this system that is **safety-relevant, individual to one
//! machine, and drifts over time**. The descriptor is signed and static; profiles are signed and
//! operator-set. Calibration is measured, it decays, and several guards are built on it.
//!
//! The failure this module exists to prevent is stated in §45.1 and it is not a dramatic one: *"a
//! camera extrinsic off by four centimetres means the `proximity` guard measures separation to the
//! wrong place while reporting perfectly healthy."* Nothing trips. Nothing looks wrong. The number
//! is simply about the world as it was last spring.
//!
//! # What this module is, and what it is not
//!
//! It answers **is this record still valid, and if not which guards lose their input**. It does not
//! compute a calibration, does not hold the values, and has no opinion about hand-eye routines —
//! §45.6 puts those in packages, so that a vendor can ship a better one without an OS release.
//!
//! Pure, like everything else in this crate: the clock is supplied and the conditions are supplied,
//! which is what lets the Hub answer "is this fleet's calibration current" from a check-in and the
//! robot answer the same question offline, by the same function.
//!
//! # Expiry degrades; it never silently continues
//!
//! §45.5, which is §7.2's rule applied to a measurement rather than to a sensor: an expired record
//! makes the guards that depend on it report **unavailable, naming the expired parameter set**. Not
//! "assume it is probably still fine", which is what every system without an expiry policy does by
//! omission — not by decision, which is the point. The robot keeps running, with fewer guards, and
//! says so on every surface.
//!
//! # A residual is a threshold, not a decoration
//!
//! §45.3: *"a hand-eye calibration with twelve millimetres of reprojection error is a failed
//! calibration, and accepting it silently produces a workspace guard that is precisely twelve
//! millimetres wrong forever."* So a record whose residual exceeds its own declared threshold is
//! [`Validity::Failed`] the moment it is read, at any age. It never becomes valid and it never
//! expires — it was never a calibration.
//!
//! # Absence is not validity
//!
//! A parameter set with no record at all is [`Validity::Absent`], and its guards are unavailable
//! for that reason. This is the same rule the whole project runs on and it matters most here,
//! because the alternative — treating "never calibrated" as "no evidence of a problem" — is what
//! puts a `proximity` guard on a robot whose camera transform is whatever the CAD model said.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

// Numbers inside these sentences are spelled by the same two rules everything else in this crate
// uses (`json`'s module header): §31.4 wants byte-identical output from every edition, and Rust's
// native `{}` for an `f64` and JavaScript's `${value}` disagree often enough that a residual would
// eventually differ by a digit in a message nobody reads until it is a support call.
use crate::json::{fixed, number_text};

/// The parameter sets §45.2 names, in the order that table gives them.
///
/// A closed vocabulary rather than a free string, for the reason §26's event codes are closed: an
/// open one is how "camera_extrinsics" and "camera-extrinsic" end up naming the same measurement on
/// two robots in one fleet, and the guard mapping below then answers differently for each.
pub const PARAMETER_SETS: [&str; 7] = [
    "joint_zeros",
    "camera_intrinsics",
    "camera_extrinsics",
    "force_torque_bias",
    "imu_bias",
    "tool_offset",
    "gravity_vector",
];

/// §45.2's table, as data: which guards lose their input when a set expires.
///
/// **This table is the deliverable of §45.2.** It converts *"we should calibrate the cameras
/// sometime"* into *"an expired extrinsic disables the proximity guard"*, which is a sentence an
/// operator can act on and a sentence a fleet console can count.
///
/// `camera_intrinsics` feeds `proximity` *indirectly* — through projection — and it is listed
/// rather than omitted: a guard whose input is two transforms away is still a guard measuring the
/// wrong thing when either is wrong, and an operator reading a list of affected guards is not
/// helped by a distinction between direct and indirect harm.
pub fn guards_fed_by(set: &str) -> &'static [&'static str] {
    match set {
        "joint_zeros" => &["workspace", "self_collision"],
        "camera_intrinsics" => &["proximity"],
        "camera_extrinsics" => &["proximity", "workspace"],
        "force_torque_bias" => &["envelope"],
        "imu_bias" => &["balance"],
        "tool_offset" => &["workspace"],
        "gravity_vector" => &["balance", "envelope"],
        _ => &[],
    }
}

/// The events §45.4 says invalidate a calibration outright.
///
/// > *"A robot that has just hit something has, by definition, possibly moved its cameras, and
/// > continuing to trust yesterday's extrinsics is exactly the wrong response to an impact."*
///
/// These are the triggers that matter, because the other three conditions are gradual and this one
/// is a step. A robot can be five minutes into a fresh calibration and one collision away from
/// having none.
pub const INVALIDATING_EVENTS: [&str; 5] = [
    "collision",
    "estop_under_load",
    "tool_change",
    "driver_replaced",
    "mainboard_swap",
];

/// When a record stops being believed (§45.4).
///
/// Every field is optional, and an absent field is **not a policy of "never"** — it is a condition
/// this record is not checked against. A record with no `max_age_s` does not expire with time; one
/// with no policy at all expires only on an event. That is a real and reportable state, which is
/// why [`Validity::Valid`] carries the conditions that were actually checked: a caller must be able
/// to tell "still valid" from "nothing was checked".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidityPolicy {
    /// Elapsed seconds since the record was taken. The backstop, and the only one that applies to
    /// everything.
    pub max_age_s: Option<f64>,
    /// Radians of joint travel since the record was taken. For joint zeros and tool offsets.
    pub max_joint_travel_rad: Option<f64>,
    /// Degrees away from the temperature the record was taken at. For F/T and IMU bias, which
    /// §45.2 says drift with temperature *continuously*.
    pub max_temperature_delta_c: Option<f64>,
    /// Events that invalidate this record outright, from [`INVALIDATING_EVENTS`].
    pub invalidating_events: Vec<String>,
}

/// The conditions a record was taken under (§45.3), and the ones it is later judged against.
///
/// The same type on both sides on purpose: an expiry rule that compares a *taken* temperature
/// against a *current* one needs both to mean the same thing, and two structs would be two places
/// for the units to diverge.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conditions {
    /// Seconds on the robot's wall clock. Wall rather than monotonic because a calibration outlives
    /// every reboot between it and its expiry, and a monotonic reading means nothing across one.
    pub at_s: Option<f64>,
    /// Temperature at the sensor, °C.
    pub temperature_c: Option<f64>,
    /// Cumulative joint travel, radians, from whatever counter the robot keeps.
    pub joint_travel_rad: Option<f64>,
    /// The tool fitted, when one was.
    pub tool: Option<String>,
    /// Payload carried, kg.
    pub payload_kg: Option<f64>,
}

/// One stored calibration (§45.3).
///
/// The **values are not here**. This module decides validity, and a record's numbers are read by
/// whatever consumes them; carrying an opaque blob through a validity check would invite the check
/// to develop opinions about content. What is here is everything §45.3 says must accompany them,
/// because *"a calibration whose method nobody recorded is a rumour with six decimal places"*.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalibrationRecord {
    /// Which parameter set, from [`PARAMETER_SETS`].
    pub set: String,
    /// The procedure that produced it: which routine, which target, which pose set.
    pub method: Option<String>,
    /// What the method yielded — reprojection error, fit RMS. Units are the method's.
    pub residual: Option<f64>,
    /// The value above which this method's own result is a failure (§45.3).
    pub residual_threshold: Option<f64>,
    /// Conditions at the time of measurement.
    pub conditions: Conditions,
    /// Who ran it.
    pub operator: Option<String>,
    /// When it stops being believed.
    pub policy: ValidityPolicy,
    /// Events recorded against this set since it was taken, from [`INVALIDATING_EVENTS`].
    pub events_since: Vec<String>,
}

/// What a record is worth right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Validity {
    /// Believed, and here is what was actually checked.
    Valid {
        /// The conditions this record was tested against. Empty means the policy checks nothing,
        /// which is a fact a report must be able to state.
        checked: Vec<String>,
    },
    /// Never a calibration: the residual its own method reported exceeds its own threshold (§45.3).
    Failed {
        /// The sentence, naming both numbers.
        detail: String,
    },
    /// It was one, and one of §45.4's conditions has since been met.
    Expired {
        /// Which condition, from [`EXPIRY_REASONS`].
        reason: &'static str,
        /// The sentence, naming the numbers that decided it.
        detail: String,
    },
    /// Nobody ever measured this set on this robot.
    Absent,
    /// A record exists and cannot be judged, because §45.3's mandatory provenance is missing.
    ///
    /// Distinct from [`Validity::Failed`]: nothing says the measurement is bad, and nothing says it
    /// is good either. §45.7's `calibration.provenance` is the conformance case that a record
    /// arriving without a method or a residual is refused rather than believed.
    Unusable {
        /// What is missing.
        detail: String,
    },
}

/// The expiry reasons, as a closed vocabulary a console can translate.
pub const EXPIRY_REASONS: [&str; 4] = ["age", "joint_travel", "temperature", "event"];

impl Validity {
    /// Whether the guards fed by this set may run.
    pub fn believed(&self) -> bool {
        matches!(self, Validity::Valid { .. })
    }

    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Validity::Valid { .. } => "valid",
            Validity::Failed { .. } => "failed",
            Validity::Expired { .. } => "expired",
            Validity::Absent => "absent",
            Validity::Unusable { .. } => "unusable",
        }
    }

    /// A sentence for an operator. Empty for a record that is simply believed.
    pub fn detail(&self) -> String {
        match self {
            Validity::Valid { .. } => String::new(),
            Validity::Failed { detail } | Validity::Unusable { detail } => detail.clone(),
            Validity::Expired { detail, .. } => detail.clone(),
            // Phrased to compose. This detail is read on its own in a table row that already names
            // the set, and inside `"the `{set}` calibration is {state} — {detail}"` in the guard
            // list, and a sentence that repeats "no record" in the second reads like two facts.
            Validity::Absent => "nobody has ever measured it on this robot".to_string(),
        }
    }
}

/// Judge one record against the present (§45.4, §45.5).
///
/// `now` is the conditions the robot is in *now*: its clock, its temperature, its travel counter.
/// Supplied rather than read, for §35.2's reason — a function that reads a clock cannot be tested
/// against a fixed one, and every expiry rule here is a comparison somebody will want to check.
///
/// # Order, and why it is this order
///
/// 1. **Provenance**, because §45.3 makes it mandatory and a record without a method cannot be
///    judged on anything else — reporting it as "valid for another 30 days" would be a claim about
///    a measurement nobody described.
/// 2. **Residual**, because a failed calibration was never valid and its age is irrelevant.
/// 3. **Events**, because §45.4 calls them the triggers that matter and they are a step rather than
///    a slope: a robot that hit something has a bad extrinsic *now*, whatever its age says.
/// 4. Then the three gradual conditions, in the order §45.4's table gives them.
///
/// A missing *current* reading does not extend validity. If the policy names a temperature delta
/// and the robot cannot say what temperature it is, the record expires with reason `temperature`
/// and a detail saying the input is missing — because the alternative is a rule that silently stops
/// applying on exactly the robot whose sensor has failed.
pub fn evaluate(record: &CalibrationRecord, now: &Conditions) -> Validity {
    // 1. §45.3's mandatory provenance. Both, and separately, because they are different omissions:
    // no method is a measurement nobody can repeat, no residual is one nobody can grade.
    let mut missing: Vec<&str> = Vec::new();
    if record.method.as_deref().unwrap_or("").is_empty() {
        missing.push("method");
    }
    if record.residual.is_none() {
        missing.push("residual");
    }
    if !missing.is_empty() {
        return Validity::Unusable {
            detail: format!(
                "§45.3 makes provenance mandatory and this record has no {}. A calibration whose \
                 method nobody recorded is a rumour with six decimal places",
                missing.join(" and no ")
            ),
        };
    }

    // 2. The residual against its own threshold.
    if let (Some(residual), Some(threshold)) = (record.residual, record.residual_threshold) {
        if residual > threshold {
            return Validity::Failed {
                detail: format!(
                    "the method reported a residual of {} against its own threshold of {}, so \
                     this was a failed calibration rather than one that has since expired",
                    number_text(residual),
                    number_text(threshold)
                ),
            };
        }
    }

    // 3. Events, which are a step rather than a slope.
    for event in &record.events_since {
        if record.policy.invalidating_events.iter().any(|e| e == event) {
            return Validity::Expired {
                reason: "event",
                detail: format!(
                    "a `{event}` was recorded against this set since it was measured, and §45.4 \
                     invalidates it outright: continuing to trust the previous values is exactly \
                     the wrong response to one"
                ),
            };
        }
    }

    let mut checked: Vec<String> = Vec::new();

    // 4. Age.
    if let Some(max_age) = record.policy.max_age_s {
        match (record.conditions.at_s, now.at_s) {
            (Some(taken), Some(current)) => {
                let age = current - taken;
                if age > max_age {
                    return Validity::Expired {
                        reason: "age",
                        detail: format!(
                            "measured {} s ago against a validity of {} s",
                            fixed(age, 0),
                            fixed(max_age, 0)
                        ),
                    };
                }
                checked.push(format!("age {}s of {}s", fixed(age, 0), fixed(max_age, 0)));
            }
            _ => {
                return Validity::Expired {
                    reason: "age",
                    detail: "this record expires with time and either it or this robot cannot say \
                             when it was taken, so its age cannot be established"
                        .to_string(),
                }
            }
        }
    }

    // 5. Joint travel.
    if let Some(max_travel) = record.policy.max_joint_travel_rad {
        match (record.conditions.joint_travel_rad, now.joint_travel_rad) {
            (Some(taken), Some(current)) => {
                let travelled = current - taken;
                if travelled > max_travel {
                    return Validity::Expired {
                        reason: "joint_travel",
                        detail: format!(
                            "{} rad of joint travel since it was measured, against a validity of \
                             {} rad",
                            fixed(travelled, 1),
                            fixed(max_travel, 1)
                        ),
                    };
                }
                checked.push(format!(
                    "travel {} of {} rad",
                    fixed(travelled, 1),
                    fixed(max_travel, 1)
                ));
            }
            _ => {
                return Validity::Expired {
                    reason: "joint_travel",
                    detail:
                        "this record expires with joint travel and no travel counter is \
                             available, so how far this robot has moved since cannot be established"
                            .to_string(),
                }
            }
        }
    }

    // 6. Temperature delta.
    if let Some(max_delta) = record.policy.max_temperature_delta_c {
        match (record.conditions.temperature_c, now.temperature_c) {
            (Some(taken), Some(current)) => {
                let delta = (current - taken).abs();
                if delta > max_delta {
                    return Validity::Expired {
                        reason: "temperature",
                        detail: format!(
                            "{} °C from the {} °C it was measured at, against a validity of {} °C",
                            fixed(delta, 1),
                            fixed(taken, 1),
                            fixed(max_delta, 1)
                        ),
                    };
                }
                checked.push(format!(
                    "temperature Δ{} of {} °C",
                    fixed(delta, 1),
                    fixed(max_delta, 1)
                ));
            }
            _ => {
                return Validity::Expired {
                    reason: "temperature",
                    detail: "this record expires with temperature and no reading is available, so \
                             how far this robot has drifted from its calibration conditions cannot \
                             be established"
                        .to_string(),
                }
            }
        }
    }

    Validity::Valid { checked }
}

/// One guard that cannot run, and the calibration that is the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardUnavailable {
    /// The guard, from the kernel's vocabulary.
    pub guard: String,
    /// The parameter set that failed it.
    pub set: String,
    /// The state of that set.
    pub state: &'static str,
    /// The sentence, naming the set as §45.5 requires.
    pub detail: String,
}

/// Which guards lose their input, given what has been measured (§45.5).
///
/// `records` is what the robot holds; every set in [`PARAMETER_SETS`] with no record is
/// [`Validity::Absent`] and takes its guards with it.
///
/// # This deliberately reports one entry per (guard, set) pair
///
/// A guard fed by two expired sets appears twice, and that is the useful shape rather than an
/// oversight: `workspace` disabled by expired joint zeros *and* an expired tool offset is two
/// measurements somebody has to take, and collapsing them to one line would hide the second until
/// the first was done.
///
/// # Only sets this robot needs
///
/// `applicable` names the sets that mean anything on this body — a fixed-base arm with no IMU
/// should not report `balance` unavailable for want of an IMU bias it can never have. Passing every
/// set is the conservative default and the honest one for a robot whose body is unknown, but it
/// produces a page of guards nobody can ever restore, which trains an operator to ignore the panel.
pub fn guards_unavailable(
    records: &[(String, Validity)],
    applicable: &[&str],
) -> Vec<GuardUnavailable> {
    let mut out = Vec::new();
    for set in applicable {
        let held = records.iter().find(|(name, _)| name == set);
        let validity = match held {
            Some((_, validity)) => validity.clone(),
            None => Validity::Absent,
        };
        if validity.believed() {
            continue;
        }
        for guard in guards_fed_by(set) {
            out.push(GuardUnavailable {
                guard: (*guard).to_string(),
                set: (*set).to_string(),
                state: validity.as_str(),
                detail: format!(
                    "the `{set}` calibration is {} — {}",
                    validity.as_str(),
                    validity.detail()
                ),
            });
        }
    }
    out
}

/// Every parameter set, for a robot whose body does not say which apply.
pub fn all_sets() -> Vec<&'static str> {
    PARAMETER_SETS.to_vec()
}

/// The sets a body can actually hold, from what it has declared.
///
/// Derived from the capability tokens and the base kind a descriptor already carries rather than
/// from a new field, so a robot does not declare the same fact twice.
///
/// # Two different absences, and they are not treated the same
///
/// **Undeclared capabilities** mean nobody said what this body senses, so every sensing-derived set
/// is included: over-reporting what is missing is the conservative direction, and quietly deciding
/// a guard was never relevant is not.
///
/// **A declared `fixed` base** is not an absence at all — it is a positive statement that this body
/// cannot fall over. `balance` is meaningless on a bolted-down arm, so an IMU bias and a gravity
/// vector are not measurements it is missing; they are measurements it will never take, and
/// reporting `balance` unavailable on one is a permanent red mark nobody can clear. §45.2 makes the
/// same point about the gravity vector directly: it drifts with *base tilt*, and a fixed base does
/// not tilt.
///
/// An **unknown** base is mobile, because that is the absence again and it goes the other way.
/// A body that does declare an IMU keeps both sets whatever its base, since it evidently has one.
pub fn sets_for(capabilities: &[String], base: Option<&str>) -> Vec<&'static str> {
    let mobile = base.map(|kind| kind != "fixed").unwrap_or(true);
    let declared = !capabilities.is_empty();
    let has = |prefix: &str| capabilities.iter().any(|c| c.starts_with(prefix));

    let mut sets = vec!["joint_zeros"];
    if !declared || has("sensing.rgb") || has("sensing.depth") || has("sensing.camera") {
        sets.push("camera_intrinsics");
        sets.push("camera_extrinsics");
    }
    if !declared || has("sensing.force") || has("manipulation") {
        sets.push("force_torque_bias");
        sets.push("tool_offset");
    }
    if mobile || has("sensing.imu") {
        sets.push("imu_bias");
        sets.push("gravity_vector");
    }
    sets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(set: &str) -> CalibrationRecord {
        CalibrationRecord {
            set: set.to_string(),
            method: Some("charuco-12x9, 40 poses".to_string()),
            residual: Some(0.4),
            residual_threshold: Some(1.0),
            conditions: Conditions {
                at_s: Some(1_000.0),
                temperature_c: Some(22.0),
                joint_travel_rad: Some(100.0),
                tool: Some("gripper-2f85".to_string()),
                payload_kg: Some(0.0),
            },
            operator: Some("alice".to_string()),
            policy: ValidityPolicy::default(),
            events_since: Vec::new(),
        }
    }

    fn now(at_s: f64) -> Conditions {
        Conditions {
            at_s: Some(at_s),
            temperature_c: Some(22.0),
            joint_travel_rad: Some(100.0),
            tool: None,
            payload_kg: None,
        }
    }

    #[test]
    fn a_record_with_no_policy_is_valid_and_says_nothing_was_checked() {
        let validity = evaluate(&record("camera_extrinsics"), &now(2_000.0));
        assert_eq!(
            validity,
            Validity::Valid {
                checked: Vec::new()
            }
        );
        // The distinction a caller needs: "still valid" and "nothing was checked" are the same
        // outcome and different facts.
        assert!(validity.believed());
    }

    /// §45.3. A residual over its own threshold was never a calibration, so it is `failed` rather
    /// than `expired` and it does not become valid at any age.
    #[test]
    fn a_residual_over_its_threshold_is_failed_and_not_expired() {
        let mut r = record("camera_extrinsics");
        r.residual = Some(12.0);
        r.residual_threshold = Some(2.0);
        r.policy.max_age_s = Some(f64::MAX);

        let validity = evaluate(&r, &now(1_001.0));
        assert_eq!(validity.as_str(), "failed");
        assert!(validity.detail().contains("12"), "{validity:?}");
        assert!(!validity.believed());
    }

    /// §45.3 makes provenance mandatory, and the two omissions are different: no method is a
    /// measurement nobody can repeat, no residual is one nobody can grade.
    #[test]
    fn a_record_with_no_provenance_is_unusable_rather_than_believed() {
        let mut r = record("joint_zeros");
        r.method = None;
        r.residual = None;
        let validity = evaluate(&r, &now(1_001.0));
        assert_eq!(validity.as_str(), "unusable");
        assert!(validity.detail().contains("method"), "{validity:?}");
        assert!(validity.detail().contains("residual"), "{validity:?}");

        let mut only_method_missing = record("joint_zeros");
        only_method_missing.method = Some(String::new());
        assert!(evaluate(&only_method_missing, &now(1_001.0))
            .detail()
            .contains("no method"));
    }

    #[test]
    fn age_expires_a_record_and_the_detail_names_both_numbers() {
        let mut r = record("camera_extrinsics");
        r.policy.max_age_s = Some(86_400.0);

        assert!(evaluate(&r, &now(1_000.0 + 80_000.0)).believed());
        let expired = evaluate(&r, &now(1_000.0 + 90_000.0));
        assert_eq!(expired.as_str(), "expired");
        assert!(matches!(expired, Validity::Expired { reason: "age", .. }));
        assert!(expired.detail().contains("86400"), "{expired:?}");
    }

    /// §45.4's step condition. A robot that has just hit something has, by definition, possibly
    /// moved its cameras — so this beats a record measured five minutes ago.
    #[test]
    fn a_collision_invalidates_a_record_taken_moments_ago() {
        let mut r = record("camera_extrinsics");
        r.policy.max_age_s = Some(86_400.0);
        r.policy.invalidating_events = vec!["collision".to_string()];
        r.events_since = vec!["collision".to_string()];

        let validity = evaluate(&r, &now(1_060.0));
        assert!(matches!(
            validity,
            Validity::Expired {
                reason: "event",
                ..
            }
        ));
        assert!(validity.detail().contains("collision"), "{validity:?}");
    }

    /// An event the policy does not list is not an expiry. A tool change does not move a camera,
    /// and expiring everything on every event would train an operator to recalibrate nothing.
    #[test]
    fn an_event_the_policy_does_not_name_leaves_a_record_alone() {
        let mut r = record("camera_extrinsics");
        r.policy.invalidating_events = vec!["collision".to_string()];
        r.events_since = vec!["tool_change".to_string()];
        assert!(evaluate(&r, &now(1_060.0)).believed());
    }

    #[test]
    fn temperature_drift_expires_a_bias_in_either_direction() {
        let mut r = record("force_torque_bias");
        r.policy.max_temperature_delta_c = Some(5.0);

        let mut cold = now(1_060.0);
        cold.temperature_c = Some(10.0);
        assert!(matches!(
            evaluate(&r, &cold),
            Validity::Expired {
                reason: "temperature",
                ..
            }
        ));

        let mut hot = now(1_060.0);
        hot.temperature_c = Some(40.0);
        assert!(matches!(
            evaluate(&r, &hot),
            Validity::Expired {
                reason: "temperature",
                ..
            }
        ));

        let mut near = now(1_060.0);
        near.temperature_c = Some(25.0);
        assert!(evaluate(&r, &near).believed());
    }

    /// The rule that keeps an expiry policy from switching itself off. A robot whose temperature
    /// sensor has failed is precisely the robot whose F/T bias should not be trusted.
    #[test]
    fn a_missing_current_reading_expires_rather_than_extends() {
        let mut r = record("force_torque_bias");
        r.policy.max_temperature_delta_c = Some(5.0);

        let mut blind = now(1_060.0);
        blind.temperature_c = None;
        let validity = evaluate(&r, &blind);
        assert!(matches!(
            validity,
            Validity::Expired {
                reason: "temperature",
                ..
            }
        ));
        assert!(validity.detail().contains("no reading"), "{validity:?}");
    }

    #[test]
    fn joint_travel_expires_zeros() {
        let mut r = record("joint_zeros");
        r.policy.max_joint_travel_rad = Some(500.0);

        let mut moved = now(1_060.0);
        moved.joint_travel_rad = Some(1_000.0);
        assert!(matches!(
            evaluate(&r, &moved),
            Validity::Expired {
                reason: "joint_travel",
                ..
            }
        ));
    }

    /// §45.2's table is the deliverable: an expired extrinsic disables the proximity guard, by
    /// name, which is a sentence an operator can act on.
    #[test]
    fn an_expired_extrinsic_takes_proximity_and_workspace_with_it() {
        let mut r = record("camera_extrinsics");
        r.policy.max_age_s = Some(60.0);
        let validity = evaluate(&r, &now(9_000.0));

        let out = guards_unavailable(
            &[("camera_extrinsics".to_string(), validity)],
            &["camera_extrinsics"],
        );
        let guards: Vec<&str> = out.iter().map(|g| g.guard.as_str()).collect();
        assert_eq!(guards, vec!["proximity", "workspace"]);
        for entry in &out {
            assert_eq!(entry.state, "expired");
            assert!(entry.detail.contains("camera_extrinsics"), "{entry:?}");
        }
    }

    /// Absence is not validity. A set nobody has ever measured disables its guards for that reason,
    /// which is the rule that keeps "never calibrated" from reading as "no evidence of a problem".
    #[test]
    fn a_set_with_no_record_disables_its_guards_as_absent() {
        let out = guards_unavailable(&[], &["imu_bias"]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].guard, "balance");
        assert_eq!(out[0].state, "absent");
        assert!(
            out[0].detail.contains("nobody has ever measured"),
            "{out:?}"
        );
    }

    /// Two expired sets feeding one guard are two measurements somebody has to take, so they are
    /// two lines. Collapsing them would hide the second until the first was done.
    #[test]
    fn a_guard_starved_by_two_sets_is_reported_twice() {
        let out = guards_unavailable(&[], &["joint_zeros", "tool_offset"]);
        let workspace: Vec<&GuardUnavailable> =
            out.iter().filter(|g| g.guard == "workspace").collect();
        assert_eq!(workspace.len(), 2);
        assert_eq!(workspace[0].set, "joint_zeros");
        assert_eq!(workspace[1].set, "tool_offset");
    }

    #[test]
    fn a_valid_record_starves_nothing() {
        let out = guards_unavailable(
            &[(
                "imu_bias".to_string(),
                Validity::Valid {
                    checked: Vec::new(),
                },
            )],
            &["imu_bias"],
        );
        assert!(out.is_empty());
    }

    /// A fixed-base arm with no IMU should not report `balance` unavailable for want of a bias it
    /// can never hold — a page of guards nobody can restore trains an operator to ignore the panel.
    #[test]
    fn the_applicable_sets_follow_the_bodys_declared_capabilities() {
        let arm = sets_for(
            &["manipulation.grasp".to_string(), "sensing.rgb".to_string()],
            Some("fixed"),
        );
        assert!(arm.contains(&"camera_extrinsics"));
        assert!(arm.contains(&"tool_offset"));
        assert!(!arm.contains(&"imu_bias"), "{arm:?}");
    }

    /// The two absences, which are not the same absence. Undeclared capabilities mean nobody said
    /// what this body senses; a declared `fixed` base is a positive statement that it cannot fall
    /// over, and reporting `balance` unavailable on a bolted-down arm is a red mark nobody can ever
    /// clear.
    #[test]
    fn a_fixed_base_retires_the_balance_sets_even_when_nothing_else_is_declared() {
        let bolted = sets_for(&[], Some("fixed"));
        assert!(!bolted.contains(&"imu_bias"), "{bolted:?}");
        assert!(!bolted.contains(&"gravity_vector"), "{bolted:?}");
        // Everything else it might sense is still included, because that part *is* undeclared.
        assert!(bolted.contains(&"camera_extrinsics"), "{bolted:?}");

        // An unknown base is mobile: the absence again, going the other way.
        assert_eq!(sets_for(&[], None).len(), PARAMETER_SETS.len());
        // And a fixed body that says it has an IMU keeps both, since it evidently has one.
        assert!(sets_for(&["sensing.imu".to_string()], Some("fixed")).contains(&"imu_bias"));
        // A legged body keeps them whatever it declares sensing.
        assert!(
            sets_for(&["manipulation.grasp".to_string()], Some("legged"))
                .contains(&"gravity_vector")
        );
    }

    /// Every set feeds at least one guard, and every guard named is one the kernel has. A table
    /// naming `proximty` would disable nothing and read as though it disabled something.
    #[test]
    fn the_guard_table_names_only_guards_the_kernel_has() {
        let kernel: Vec<&str> = skynet_kernel::GUARDS.iter().map(|g| g.as_str()).collect();
        for set in PARAMETER_SETS {
            let fed = guards_fed_by(set);
            assert!(!fed.is_empty(), "{set} feeds no guard");
            for guard in fed {
                assert!(
                    kernel.contains(guard),
                    "{set} names `{guard}`, which is not a guard"
                );
            }
        }
    }
}
