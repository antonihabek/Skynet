//! Serialising contract outputs, and reading corpus inputs.
//!
//! Everything an edition is measured on passes through here. §31.4 requires **byte-identical**
//! output rather than equivalent output, so this module is where the shapes are pinned: which
//! fields appear, which are omitted when absent, and how a number inside an English sentence is
//! spelled.
//!
//! # Numbers inside sentences
//!
//! Two different rules, and conflating them would produce output that differs from the reference
//! by a digit in a message nobody reads until it is a support call.
//!
//! * [`number_text`] is ECMAScript `ToString(Number)` — what `${value}` produces in the reference
//!   implementation. Positional in `[1e-6, 1e21)`, exponential outside it.
//! * [`fixed`] is ECMAScript `Number.prototype.toFixed` — round to a fixed number of decimals,
//!   ties away from zero.
//!
//! [`skynet_json::format_number`] is a third rule again, for JSON *values* rather than sentences:
//! always positional, shortest round-trip. Three rules sounds like two too many, and it is the
//! honest count — the alternative is one rule that silently disagrees with the reference in two of
//! the three places it is used.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use skynet_json::{format_number, Json};

use crate::admission::*;
use crate::calibration::{
    self, CalibrationRecord, Conditions, GuardUnavailable, Validity, ValidityPolicy,
};
use crate::certification::*;
use crate::embodiment::*;
use crate::live::{BasePose, CalibrationFact, DriveState, JointReading, LiveInputs};
use crate::opstate::OperatingState;
use crate::ota::{BenchPass, BootEvidence, BundleManifest, UpdateSubject, DEFAULT_BOOT_BUDGET_MS};
use crate::power::PowerProfile;
use crate::readiness::*;
use crate::roles::ParsedRole;
use crate::transport::*;

// ── Numbers in sentences ─────────────────────────────────────────────────────

/// ECMAScript `ToString(Number)`.
///
/// The reference implementation interpolates numbers into warning messages with `${value}`, and
/// those messages are compared byte for byte, so this reproduces that spelling exactly: positional
/// notation for magnitudes in `[1e-6, 1e21)`, exponential outside it with a signed exponent.
pub fn number_text(value: f64) -> String {
    if !value.is_finite() {
        return if value.is_nan() {
            "NaN".to_string()
        } else if value > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    if value == 0.0 {
        return "0".to_string();
    }

    let magnitude = value.abs();
    if (1e-6..1e21).contains(&magnitude) {
        // Inside this band ECMAScript uses positional notation, which is what `format_number`
        // always produces.
        return format_number(value).unwrap_or_else(|_| "0".to_string());
    }

    // Outside it, ECMAScript switches to exponential: shortest mantissa, `e`, an explicit sign,
    // and no leading zero on the exponent.
    let mut precision = 1usize;
    let mut rendered = render_exponential(value, 0);
    while precision < 17 {
        if rendered.parse::<f64>().map(f64::to_bits) == Ok(value.to_bits()) {
            break;
        }
        precision += 1;
        rendered = render_exponential(value, precision - 1);
    }

    let (mantissa, exponent) = rendered.split_once('e').unwrap_or((rendered.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{}", exponent.abs())
}

fn render_exponential(value: f64, fraction_digits: usize) -> String {
    let mut s = String::new();
    let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{:.*e}", fraction_digits, value));
    s
}

/// ECMAScript `Number.prototype.toFixed`.
///
/// Rounds to `digits` decimals, picking the larger magnitude on an exact tie — the reference uses
/// it to render a scale as a whole percentage and a field-conformance ratio to three decimals.
///
/// `powi`, `floor` and `round` all live in `std` rather than `core`, and this crate is `no_std` so
/// the same contract can run on a robot with no operating system under it. The arithmetic below is
/// exact for the magnitudes involved: a percentage, and never more than three decimal places.
pub fn fixed(value: f64, digits: u32) -> String {
    if !value.is_finite() {
        return number_text(value);
    }
    let negative = value < 0.0;
    let magnitude = value.abs();

    let mut scale = 1.0f64;
    for _ in 0..digits {
        scale *= 10.0;
    }
    let scaled = magnitude * scale;
    // Truncation toward zero equals `floor` for a non-negative value.
    let truncated = (scaled as i64) as f64;
    let fraction = scaled - truncated;
    // "If there are two such n, pick the larger n" — the tie goes up in magnitude.
    let n = if fraction >= 0.5 {
        truncated + 1.0
    } else {
        truncated
    } as i64;

    let mut digits_text = n.to_string();
    if digits > 0 {
        let width = digits as usize + 1;
        while digits_text.len() < width {
            digits_text.insert(0, '0');
        }
        digits_text.insert(digits_text.len() - digits as usize, '.');
    }
    if negative && n != 0 {
        digits_text.insert(0, '-');
    }
    digits_text
}

/// `toFixed(0)`, the spelling used for a scaling percentage.
pub fn fixed0(value: f64) -> String {
    fixed(value, 0)
}

// ── Small helpers ────────────────────────────────────────────────────────────

fn opt_num(value: Option<f64>) -> Option<Json> {
    value.map(Json::Num)
}

fn opt_str(value: &Option<String>) -> Option<Json> {
    value.as_ref().map(|s| Json::Str(s.clone()))
}

fn str_list(values: &[String]) -> Json {
    Json::Arr(values.iter().map(|s| Json::Str(s.clone())).collect())
}

fn nullable_str(value: &Option<String>) -> Json {
    match value {
        Some(text) => Json::Str(text.clone()),
        None => Json::Null,
    }
}

fn nullable_num(value: Option<f64>) -> Json {
    match value {
        Some(number) => Json::Num(number),
        None => Json::Null,
    }
}

// ── Roles ────────────────────────────────────────────────────────────────────

/// Serialise a parse result. `null` for a role the vocabulary does not describe.
pub fn role_to_json(parsed: &Option<ParsedRole>) -> Json {
    let Some(parsed) = parsed else {
        return Json::Null;
    };
    let mut out = Json::obj();
    out.set("chain", Json::Str(parsed.chain.clone()));
    out.set("link", Json::Str(parsed.link.clone()));
    out.set(
        "axis",
        match &parsed.axis {
            Some(axis) => Json::Str(axis.clone()),
            None => Json::Null,
        },
    );
    out.set(
        "index",
        match parsed.index {
            Some(index) => Json::Num(index as f64),
            None => Json::Null,
        },
    );
    out.set("key", Json::Str(parsed.key()));
    out
}

// ── Transport ────────────────────────────────────────────────────────────────

/// Serialise a transport plan.
pub fn plan_to_json(plan: &TransportPlan) -> Json {
    let mut out = Json::obj();
    out.set("verdict", Json::from(plan.verdict.as_str()));
    out.set("robot_slug", Json::Str(plan.robot_slug.clone()));
    out.set("source_digest", nullable_str(&plan.source_digest));
    out.set("target_digest", nullable_str(&plan.target_digest));
    out.set("control_mode", Json::Str(plan.control_mode.clone()));
    out.set("control_hz", nullable_num(plan.control_hz));

    let mut bindings = Json::arr();
    for binding in &plan.bindings {
        let mut item = Json::obj();
        item.set("source_index", Json::Num(binding.source_index));
        item.set("source_role", nullable_str(&binding.source_role));
        item.set("method", Json::from(binding.method.as_str()));
        item.set("target", nullable_str(&binding.target));
        item.set("scale", Json::Num(binding.scale));
        item.set("limit", nullable_num(binding.limit));
        item.set("reason", Json::Str(binding.reason.clone()));
        bindings.push(item);
    }
    out.set("bindings", bindings);

    let mut observations = Json::arr();
    for observation in &plan.observations {
        let mut item = Json::obj();
        item.set("required", Json::Str(observation.required.clone()));
        item.set("method", Json::from(observation.method.as_str()));
        item.set("target", nullable_str(&observation.target));
        item.set("reason", Json::Str(observation.reason.clone()));
        observations.push(item);
    }
    out.set("observations", observations);

    out.set(
        "clamp",
        match &plan.clamp {
            None => Json::Null,
            Some(clamp) => {
                let mut item = Json::obj();
                // Optional ceilings are *omitted* when absent rather than written as null, because
                // "this robot declares no torque ceiling" and "this robot's torque ceiling is
                // null" would otherwise be the same sentence.
                item.set_opt(
                    "joint_velocity_max_rad_s",
                    opt_num(clamp.joint_velocity_max_rad_s),
                );
                item.set_opt("joint_torque_max_nm", opt_num(clamp.joint_torque_max_nm));
                item.set_opt(
                    "base_velocity_max_m_s",
                    opt_num(clamp.base_velocity_max_m_s),
                );
                item.set_opt("min_control_hz", opt_num(clamp.min_control_hz));
                let mut per_channel = Json::obj();
                for (name, limit) in &clamp.per_channel {
                    per_channel.set(name, Json::Num(*limit));
                }
                item.set("per_channel", per_channel);
                item.set_opt("source", opt_str(&clamp.source));
                item
            }
        },
    );

    let mut warnings = Json::arr();
    for warning in &plan.warnings {
        let mut item = Json::obj();
        item.set("code", Json::from(warning.code.as_str()));
        item.set("message", Json::Str(warning.message.clone()));
        item.set_opt("subjects", warning.subjects.as_ref().map(|s| str_list(s)));
        warnings.push(item);
    }
    out.set("warnings", warnings);

    out
}

// ── Admission ────────────────────────────────────────────────────────────────

/// Serialise an admission result.
pub fn admission_to_json(result: &AdmissionResult) -> Json {
    let mut out = Json::obj();
    out.set("admitted", Json::Bool(result.admitted));
    let mut findings = Json::arr();
    for finding in &result.findings {
        let mut item = Json::obj();
        item.set("code", Json::from(finding.code.as_str()));
        item.set("severity", Json::from(finding.severity.as_str()));
        item.set("message", Json::Str(finding.message.clone()));
        item.set_opt("subjects", finding.subjects.as_ref().map(|s| str_list(s)));
        findings.push(item);
    }
    out.set("findings", findings);
    out.set("effective_grants", str_list(&result.effective_grants));
    out
}

// ── Certification ────────────────────────────────────────────────────────────

/// Serialise a certification verdict.
pub fn certification_to_json(verdict: &CertificationVerdict) -> Json {
    let mut out = Json::obj();
    out.set("awarded", Json::Str(verdict.awarded.clone()));
    out.set("failures", str_list(&verdict.failures));
    out.set("missing", str_list(&verdict.missing));
    out.set("reasons", str_list(&verdict.reasons));
    out.set("expires_in_days", nullable_num(verdict.expires_in_days));
    out
}

// ── Readiness ────────────────────────────────────────────────────────────────

fn check_to_json(check: &CheckResult) -> Json {
    let mut item = Json::obj();
    item.set("check", Json::from(check.check));
    item.set("ok", Json::Bool(check.ok));
    item.set("required_for", Json::from(check.required_for));
    item.set("detail", Json::Str(check.detail.clone()));
    item
}

/// Serialise a readiness report.
pub fn readiness_to_json(report: &ReadinessReport) -> Json {
    let mut out = Json::obj();
    out.set("slug", Json::Str(report.slug.clone()));
    out.set("name", Json::Str(report.name.clone()));
    out.set("attainable", nullable_str(&report.attainable));
    out.set(
        "checks",
        Json::Arr(report.checks.iter().map(check_to_json).collect()),
    );
    out.set(
        "blockers",
        Json::Arr(report.blockers.iter().map(check_to_json).collect()),
    );
    out.set("completeness", Json::Num(report.completeness));
    out
}

/// Serialise a fleet summary.
pub fn fleet_to_json(fleet: &FleetReadiness) -> Json {
    let mut out = Json::obj();
    out.set("total", Json::from(fleet.total));
    let mut by_tier = Json::obj();
    for (tier, count) in &fleet.by_tier {
        by_tier.set(tier, Json::from(*count));
    }
    out.set("by_tier", by_tier);
    let mut blockers = Json::arr();
    for (check, robots) in &fleet.common_blockers {
        let mut item = Json::obj();
        item.set("check", Json::from(*check));
        item.set("robots", Json::from(*robots));
        blockers.push(item);
    }
    out.set("common_blockers", blockers);
    out
}

// ── Reading corpus inputs ────────────────────────────────────────────────────

/// What went wrong reading a case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn require<'a>(value: &'a Json, key: &str) -> Result<&'a Json, DecodeError> {
    value
        .get(key)
        .ok_or_else(|| DecodeError(format!("missing field `{key}`")))
}

fn req_str(value: &Json, key: &str) -> Result<String, DecodeError> {
    require(value, key)?
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| DecodeError(format!("field `{key}` is not a string")))
}

fn req_num(value: &Json, key: &str) -> Result<f64, DecodeError> {
    require(value, key)?
        .as_f64()
        .ok_or_else(|| DecodeError(format!("field `{key}` is not a number")))
}

fn opt_string(value: &Json, key: &str) -> Option<String> {
    value.get_present(key)?.as_str().map(ToString::to_string)
}

fn opt_f64(value: &Json, key: &str) -> Option<f64> {
    value.get_present(key)?.as_f64()
}

fn flag(value: &Json, key: &str) -> bool {
    value.get(key).and_then(Json::as_bool).unwrap_or(false)
}

/// Read a [`SafetyEnvelope`]. `None` for an absent or explicitly null field.
pub fn envelope_from_json(value: &Json, key: &str) -> Option<SafetyEnvelope> {
    let node = value.get_present(key)?;
    Some(SafetyEnvelope {
        joint_velocity_max_rad_s: opt_f64(node, "joint_velocity_max_rad_s"),
        joint_torque_max_nm: opt_f64(node, "joint_torque_max_nm"),
        base_velocity_max_m_s: opt_f64(node, "base_velocity_max_m_s"),
        watchdog_ms: opt_f64(node, "watchdog_ms"),
        estop: node.get_present("estop").and_then(Json::as_bool),
        source: opt_string(node, "source"),
    })
}

/// Read an [`EmbodimentDescriptor`].
pub fn descriptor_from_json(value: &Json) -> Result<EmbodimentDescriptor, DecodeError> {
    let control = value.get("control").cloned().unwrap_or_else(Json::obj);

    let mut joints = Vec::new();
    for item in value.arr_or_empty("joints") {
        let limits = item.get("limits").cloned().unwrap_or_else(Json::obj);
        joints.push(EmbodimentJoint {
            name: req_str(item, "name")?,
            role: opt_string(item, "role"),
            joint_type: opt_string(item, "type").unwrap_or_else(|| "revolute".to_string()),
            limits: JointLimits {
                position_min_rad: opt_f64(&limits, "position_min_rad"),
                position_max_rad: opt_f64(&limits, "position_max_rad"),
                velocity_max_rad_s: opt_f64(&limits, "velocity_max_rad_s"),
                effort_max_nm: opt_f64(&limits, "effort_max_nm"),
            },
            control_modes: item.str_arr("control_modes"),
        });
    }

    let mut end_effectors = Vec::new();
    for item in value.arr_or_empty("end_effectors") {
        end_effectors.push(EmbodimentEndEffector {
            name: req_str(item, "name")?,
            role: opt_string(item, "role"),
            kind: req_str(item, "kind")?,
            dof: opt_f64(item, "dof").unwrap_or(0.0),
            max_grip_force_n: opt_f64(item, "max_grip_force_n"),
            max_opening_m: opt_f64(item, "max_opening_m"),
        });
    }

    let mut sensors = Vec::new();
    for item in value.arr_or_empty("sensors") {
        sensors.push(EmbodimentSensor {
            name: req_str(item, "name")?,
            role: opt_string(item, "role"),
            modality: req_str(item, "modality")?,
            width: opt_f64(item, "width"),
            height: opt_f64(item, "height"),
            fps: opt_f64(item, "fps"),
            frame: opt_string(item, "frame"),
            latency_ms: opt_f64(item, "latency_ms"),
            // §5's three states survive because the field is `Option<bool>`: absent and an
            // explicit `null` are both `None`, and `Some(false)` is a vendor who answered the
            // question. `as_bool` is what makes that safe — it refuses a string and a number
            // rather than coercing either, so a malformed declaration reads as undeclared and
            // leaves `proximity` unavailable rather than active on an unestablished claim.
            detects_people: item.get_present("detects_people").and_then(Json::as_bool),
        });
    }

    Ok(EmbodimentDescriptor {
        schema_version: value
            .get("schema_version")
            .and_then(Json::as_usize)
            .unwrap_or(1) as u32,
        robot_slug: req_str(value, "robot_slug")?,
        digest: opt_string(value, "digest"),
        verified: flag(value, "verified"),
        base: req_str(value, "base")?,
        dof: req_num(value, "dof")?,
        joints,
        end_effectors,
        sensors,
        control: ControlProfile {
            modes: control.str_arr("modes"),
            rate_hz_max: opt_f64(&control, "rate_hz_max"),
            rate_hz_min: opt_f64(&control, "rate_hz_min"),
            command_latency_ms: opt_f64(&control, "command_latency_ms"),
        },
        envelope: envelope_from_json(value, "envelope"),
        solvers: value.str_arr("solvers"),
        // Absent reads as empty, and empty means "provides nothing" rather than "nobody said".
        // A descriptor written before this field existed therefore provides no capabilities and
        // admission refuses packages needing one — which is the direction that fails safe, and is
        // visible rather than silent because the refusal names the tokens.
        capabilities: value.str_arr("capabilities"),
        extensions: value.str_arr("extensions"),
        simulator: flag(value, "simulator"),
        // §46.2. Read as declared, validated nowhere here: an unrecognised value is *not* silently
        // dropped to `None`, because "this body says `freewheel` and nobody knows what that means"
        // and "this body says nothing" are different facts with different remedies, and the
        // readiness check names whichever it finds.
        on_command_loss: opt_string(value, "on_command_loss"),
        watchdog: value.get_present("watchdog").map(|block| DeclaredWindows {
            soc_wdt_ms: opt_f64(block, "soc_wdt_ms"),
            drive_command_timeout_ms: opt_f64(block, "drive_command_timeout_ms"),
        }),
        // §36. `brownout_fault_codes` is read through `as_usize` rather than `as_f64`: a fault code
        // is an integer the vendor writes down, and `4097.5` is a typo rather than a code between
        // two codes. A non-integer entry is dropped rather than rounded — matching one is what
        // makes a robot stop, and a stop caused by rounding is a stop nobody can explain.
        power: value.get_present("power").map(|block| PowerProfile {
            time_to_safe_state_s: opt_f64(block, "time_to_safe_state_s"),
            safe_stop_energy_wh: opt_f64(block, "safe_stop_energy_wh"),
            pack_capacity_wh: opt_f64(block, "pack_capacity_wh"),
            thermal_throttle_c: opt_f64(block, "thermal_throttle_c"),
            thermal_critical_c: opt_f64(block, "thermal_critical_c"),
            brownout_fault_codes: block
                .arr_or_empty("brownout_fault_codes")
                .iter()
                .filter_map(|code| code.as_usize())
                .map(|code| code as u32)
                .collect(),
        }),
        frames: frames_from_json(value.get("frames")),
    })
}

/// The wire shape of a frame-graph verdict.
///
/// `fault` is present-and-null rather than absent when the graph is usable: "this graph is usable"
/// is a fact worth stating, and a consumer reading an absent key cannot tell it from a producer
/// that forgot to run the check.
pub fn frame_fault_to_json(fault: &Option<FrameFault>) -> Json {
    let mut out = Json::obj();
    match fault {
        None => {
            out.set("fault", Json::Null);
        }
        Some(fault) => {
            let mut body = Json::obj();
            body.set("code", Json::Str(fault.code.clone()))
                .set(
                    "subjects",
                    Json::Arr(fault.subjects.iter().cloned().map(Json::Str).collect()),
                )
                .set("detail", Json::Str(fault.detail.clone()));
            out.set("fault", body);
        }
    }
    out
}

/// Read a [`FrameGraph`] (§44.3).
///
/// Absent and null both produce `None`, which is the same fact to every consumer: there is no tree
/// to compute kinematics from. A malformed one produces a graph that [`validate_frames`] then
/// rejects with the offending frame named, rather than a decode error with a byte offset — the
/// caller needs to tell an operator which frame is wrong, not which character.
pub fn frames_from_json(value: Option<&Json>) -> Option<FrameGraph> {
    let value = value?;
    if matches!(value, Json::Null) {
        return None;
    }

    let mut nodes = Vec::new();
    for item in value.arr_or_empty("nodes") {
        nodes.push(FrameNode {
            name: opt_string(item, "name").unwrap_or_default(),
            parent: opt_string(item, "parent"),
            kind: opt_string(item, "kind").unwrap_or_else(|| "static".to_string()),
            translation: vec3(item, "translation"),
            rotation: vec4(item, "rotation"),
            source: opt_string(item, "source"),
            joint: opt_string(item, "joint"),
            joint_type: opt_string(item, "joint_type"),
            axis: vec3(item, "axis"),
            mimics: opt_string(item, "mimics"),
            mimic_multiplier: opt_f64(item, "mimic_multiplier"),
            mimic_offset: opt_f64(item, "mimic_offset"),
            collision: flag(item, "collision"),
            collision_shape: collision_shape_from_json(item.get_present("collision_shape")),
            collision_ignore: item.str_arr("collision_ignore"),
            mass_kg: opt_f64(item, "mass_kg"),
        });
    }

    Some(FrameGraph {
        root: opt_string(value, "root").unwrap_or_default(),
        convention: opt_string(value, "convention").unwrap_or_default(),
        generator: opt_string(value, "generator"),
        nodes,
    })
}

/// Read a frame's collision geometry (§44.3).
///
/// A shape whose `kind` is unrecognised is decoded and carried rather than dropped, because
/// [`validate_frames`] refuses it by name and a silently dropped shape is a limb that disappears
/// from the collision test with nobody told.
fn collision_shape_from_json(value: Option<&Json>) -> Option<CollisionShape> {
    let value = value?;
    if matches!(value, Json::Null) {
        return None;
    }
    Some(CollisionShape {
        kind: opt_string(value, "kind").unwrap_or_default(),
        a: vec3(value, "a").unwrap_or([0.0, 0.0, 0.0]),
        b: vec3(value, "b").unwrap_or([0.0, 0.0, 0.0]),
        // `NAN` rather than a plausible default: a shape with no radius bounds nothing, and
        // `validate_frames` refuses it. Inventing a radius here is exactly the "invent a number"
        // failure the whole contract layer exists to prevent.
        radius_m: opt_f64(value, "radius_m").unwrap_or(f64::NAN),
    })
}

fn vec3(value: &Json, key: &str) -> Option<[f64; 3]> {
    let items = value.get(key)?.as_arr()?;
    if items.len() != 3 {
        return None;
    }
    Some([items[0].as_f64()?, items[1].as_f64()?, items[2].as_f64()?])
}

fn vec4(value: &Json, key: &str) -> Option<[f64; 4]> {
    let items = value.get(key)?.as_arr()?;
    if items.len() != 4 {
        return None;
    }
    Some([
        items[0].as_f64()?,
        items[1].as_f64()?,
        items[2].as_f64()?,
        items[3].as_f64()?,
    ])
}

/// Read a [`SourceEmbodiment`].
pub fn source_from_json(value: &Json) -> Result<SourceEmbodiment, DecodeError> {
    let mut action_channels = Vec::new();
    for item in value.arr_or_empty("action_channels") {
        action_channels.push(ActionChannel {
            index: req_num(item, "index")?,
            kind: req_str(item, "kind")?,
            role: opt_string(item, "role"),
            joint: opt_string(item, "joint"),
            unit: opt_string(item, "unit"),
            peak: opt_f64(item, "peak"),
            optional: flag(item, "optional"),
        });
    }

    let mut observations = Vec::new();
    for item in value.arr_or_empty("observations") {
        observations.push(ObservationRequirement {
            name: req_str(item, "name")?,
            modality: req_str(item, "modality")?,
            role: opt_string(item, "role"),
            width: opt_f64(item, "width"),
            height: opt_f64(item, "height"),
            fps: opt_f64(item, "fps"),
            optional: flag(item, "optional"),
        });
    }

    Ok(SourceEmbodiment {
        robot_slug: opt_string(value, "robot_slug"),
        digest: opt_string(value, "digest"),
        dof: opt_f64(value, "dof"),
        control_mode: req_str(value, "control_mode")?,
        control_hz: opt_f64(value, "control_hz"),
        action_channels,
        observations,
    })
}

/// Read [`ResolveTransportOptions`].
pub fn options_from_json(value: Option<&Json>) -> ResolveTransportOptions {
    let Some(value) = value else {
        return ResolveTransportOptions::default();
    };
    // Both spellings are accepted so a corpus case can be written in either the reference
    // implementation's camelCase or the wire snake_case without meaning two different things.
    ResolveTransportOptions {
        allow_retarget: flag(value, "allowRetarget") || flag(value, "allow_retarget"),
        requested_hz: opt_f64(value, "requestedHz").or_else(|| opt_f64(value, "requested_hz")),
    }
}

/// Read a [`PackageManifest`].
pub fn manifest_from_json(value: &Json) -> Result<PackageManifest, DecodeError> {
    let budget = value.get("budget").cloned().unwrap_or_else(Json::obj);
    let embodiment = value.get("embodiment").cloned().unwrap_or_else(Json::obj);

    let mut depends_on = Vec::new();
    for item in value.arr_or_empty("depends_on") {
        depends_on.push(Dependency {
            id: req_str(item, "id")?,
            range: req_str(item, "range")?,
        });
    }

    Ok(PackageManifest {
        schema_version: value
            .get("schema_version")
            .and_then(Json::as_usize)
            .unwrap_or(1) as u32,
        id: req_str(value, "id")?,
        version: req_str(value, "version")?,
        kind: req_str(value, "kind")?,
        digest: req_str(value, "digest")?,
        isolation: req_str(value, "isolation")?,
        budget: ResourceBudget {
            ram_mb: opt_f64(&budget, "ram_mb").unwrap_or(0.0),
            vram_mb: opt_f64(&budget, "vram_mb"),
            cpu_cores: opt_f64(&budget, "cpu_cores"),
            disk_mb: opt_f64(&budget, "disk_mb"),
            control_hz: opt_f64(&budget, "control_hz"),
            inference_budget_ms: opt_f64(&budget, "inference_budget_ms"),
        },
        embodiment: EmbodimentRequirement {
            capabilities: embodiment.str_arr("capabilities"),
            extensions: embodiment.str_arr("extensions"),
            control_mode: opt_string(&embodiment, "control_mode").unwrap_or_default(),
            min_dof: opt_f64(&embodiment, "min_dof"),
            base_kinds: embodiment.str_arr("base_kinds"),
            verified_on: embodiment.str_arr("verified_on"),
        },
        requests: value.str_arr("requests"),
        runtime: value.get_present("runtime").map(|runtime| RuntimeSpec {
            loader: opt_string(runtime, "loader"),
            entrypoint: opt_string(runtime, "entrypoint"),
            config: opt_string(runtime, "config"),
            container_image: opt_string(runtime, "container_image"),
        }),
        depends_on,
        // §6, for §37.1's `transport` stage. Read from `source_embodiment`, and a block that does
        // not decode is an **error** rather than an absence: an action space nobody could parse and
        // an action space nobody declared are opposite facts, and the second is a package that
        // transports trivially while the first is a package nobody has checked.
        source_embodiment: match value.get_present("source_embodiment") {
            Some(block) => Some(source_from_json(block)?),
            None => None,
        },
        min_os_version: opt_string(value, "min_os_version"),
        // Read from the top level, where the Hub's card schema puts them, and never merged into
        // `embodiment` above. Merging would silently repair a manifest whose two declarations
        // disagree, which is the one thing admission has to be able to see.
        required_capabilities: value.str_arr("required_capabilities"),
        required_extensions: value.str_arr("required_extensions"),
    })
}

/// Read a [`DeviceCapacity`].
pub fn capacity_from_json(value: &Json) -> Result<DeviceCapacity, DecodeError> {
    let mut installed = Vec::new();
    for item in value.arr_or_empty("installed") {
        installed.push(InstalledPackage {
            id: req_str(item, "id")?,
            version: req_str(item, "version")?,
        });
    }
    Ok(DeviceCapacity {
        ram_mb: opt_f64(value, "ram_mb").unwrap_or(0.0),
        vram_mb: opt_f64(value, "vram_mb"),
        cpu_cores: opt_f64(value, "cpu_cores"),
        disk_mb: opt_f64(value, "disk_mb"),
        os_version: req_str(value, "os_version")?,
        capabilities: value.str_arr("capabilities"),
        extensions: value.str_arr("extensions"),
        granted: value.str_arr("granted"),
        denied: value.str_arr("denied"),
        installed,
        // Absent is `false`, which is §52's rule for this flag rather than this reader being
        // lenient: a physical machine's existence answers the question.
        simulator: value
            .get("simulator")
            .and_then(Json::as_bool)
            .unwrap_or(false),
    })
}

/// Read an [`AdmissionParams`].
pub fn admission_params_from_json(value: &Json) -> Result<AdmissionParams, DecodeError> {
    Ok(AdmissionParams {
        manifest: manifest_from_json(require(value, "manifest")?)?,
        capacity: capacity_from_json(require(value, "capacity")?)?,
        // No default. `flag` read a missing key as `false`, which is exactly the conflation this
        // field was changed to remove — an absent state would have silently become "unsigned"
        // rather than "nobody said". A document that does not name one is refused.
        signature: {
            let text = req_str(value, "signature")?;
            SignatureState::parse(&text).ok_or_else(|| {
                DecodeError(format!(
                    "signature: `{text}` is not one of verified, failed, absent, unverifiable"
                ))
            })?
        },
        observed_digest: req_str(value, "observed_digest")?,
        transport_verdict: opt_string(value, "transport_verdict"),
        executes_on_load: flag(value, "executes_on_load"),
        trust_remote_code: flag(value, "trust_remote_code"),
    })
}

/// Read a [`CertificationInput`].
pub fn certification_input_from_json(value: &Json) -> Result<CertificationInput, DecodeError> {
    let mut results = Vec::new();
    for item in value.arr_or_empty("results") {
        let outcome_text = req_str(item, "outcome")?;
        let outcome = SctsOutcome::parse(&outcome_text)
            .ok_or_else(|| DecodeError(format!("unknown outcome `{outcome_text}`")))?;
        results.push(SctsResult {
            test_id: req_str(item, "test_id")?,
            outcome,
            value: opt_f64(item, "value"),
            ran_at: opt_string(item, "ran_at").unwrap_or_default(),
            detail: opt_string(item, "detail"),
        });
    }
    Ok(CertificationInput {
        claimed: req_str(value, "claimed")?,
        results,
        field_conformance: opt_f64(value, "field_conformance"),
        bench_age_days: opt_f64(value, "bench_age_days"),
        simulator: flag(value, "simulator"),
    })
}

/// Read a [`ReadinessSubject`].
pub fn readiness_subject_from_json(value: &Json) -> Result<ReadinessSubject, DecodeError> {
    let compute_keys = match value.get_present("compute").and_then(Json::as_obj) {
        Some(map) => map.keys().cloned().collect(),
        None => Vec::new(),
    };
    Ok(ReadinessSubject {
        slug: req_str(value, "slug")?,
        name: req_str(value, "name")?,
        dof: opt_f64(value, "dof"),
        capabilities: value.str_arr("capabilities"),
        control_modes: value.str_arr("control_modes"),
        control_hz: opt_f64(value, "control_hz"),
        compute_keys,
        limits: envelope_from_json(value, "limits"),
        adapter_count: opt_f64(value, "adapter_count").unwrap_or(0.0),
    })
}

// ── Digests (§37.1) ──────────────────────────────────────────────────────────

/// Write a digest outcome.
///
/// Shaped as an outcome rather than as a `String` that might be empty, because "this package hashes
/// to X" and "this package has no computable identity" are the two answers an installer acts on
/// differently and a caller must not be able to confuse them. `error.subject` names the offending
/// thing — a path, a dotted field, a key — and `error.detail` carries a reason only where the kind
/// alone does not identify one, which is the path rules.
pub fn digest_to_json(outcome: &Result<String, crate::digest::DigestError>) -> Json {
    use crate::digest::DigestError as E;

    let mut out = Json::obj();
    match outcome {
        Ok(digest) => {
            out.set("digest", Json::Str(digest.clone()))
                .set("error", Json::Null);
        }
        Err(error) => {
            let (kind, subject, detail) = match error {
                E::Path { path, detail } => {
                    ("path", Some(path.clone()), Some((*detail).to_string()))
                }
                E::DuplicatePath { path } => ("duplicate_path", Some(path.clone()), None),
                E::Sha256 { path, .. } => ("sha256", Some(path.clone()), None),
                E::Manifest { detail } => ("manifest", None, Some(detail.clone())),
                E::NumberOutOfRange { field, .. } => {
                    ("number_out_of_range", Some(field.clone()), None)
                }
                E::KeyOutsideBmp { at, key } => {
                    ("key_outside_bmp", Some(format!("{at} :: {key}")), None)
                }
            };
            let mut body = Json::obj();
            body.set("kind", Json::Str(kind.to_string()))
                .set("subject", subject.map_or(Json::Null, Json::Str))
                .set("detail", detail.map_or(Json::Null, Json::Str));
            out.set("digest", Json::Null).set("error", body);
        }
    }
    out
}

/// Read a [`crate::digest::DigestFile`] list from a corpus case.
pub fn digest_files_from_json(value: Option<&Json>) -> Vec<crate::digest::DigestFile> {
    let Some(value) = value else {
        return Vec::new();
    };
    value
        .as_arr()
        .unwrap_or(&[])
        .iter()
        .map(|item| crate::digest::DigestFile {
            path: item
                .get_present("path")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            sha256: item
                .get_present("sha256")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

// ── Calibration (§45) ────────────────────────────────────────────────────────

/// Read the conditions a record was taken under, or the ones a robot is in now.
pub fn conditions_from_json(value: Option<&Json>) -> Conditions {
    let Some(value) = value else {
        return Conditions::default();
    };
    Conditions {
        at_s: opt_f64(value, "at_s"),
        temperature_c: opt_f64(value, "temperature_c"),
        joint_travel_rad: opt_f64(value, "joint_travel_rad"),
        tool: opt_string(value, "tool"),
        payload_kg: opt_f64(value, "payload_kg"),
    }
}

/// Read a validity policy (§45.4).
fn policy_from_json(value: Option<&Json>) -> ValidityPolicy {
    let Some(value) = value else {
        return ValidityPolicy::default();
    };
    ValidityPolicy {
        max_age_s: opt_f64(value, "max_age_s"),
        max_joint_travel_rad: opt_f64(value, "max_joint_travel_rad"),
        max_temperature_delta_c: opt_f64(value, "max_temperature_delta_c"),
        invalidating_events: value.str_arr("invalidating_events"),
    }
}

/// Read one stored calibration (§45.3).
///
/// `set` is the one required field, because a record that does not say which parameter set it is
/// cannot be filed, let alone judged. Everything else is optional *here* and mandatory in
/// [`calibration::evaluate`] — the decoder's job is to report what the document says, and the
/// verdict's job is to refuse it. Merging the two would make a record with no method a parse error,
/// which is the wrong sentence: the file is fine and the calibration is a rumour.
pub fn calibration_from_json(value: &Json) -> Result<CalibrationRecord, DecodeError> {
    Ok(CalibrationRecord {
        set: req_str(value, "set")?,
        method: opt_string(value, "method"),
        residual: opt_f64(value, "residual"),
        residual_threshold: opt_f64(value, "residual_threshold"),
        conditions: conditions_from_json(value.get_present("conditions")),
        operator: opt_string(value, "operator"),
        policy: policy_from_json(value.get_present("policy")),
        events_since: value.str_arr("events_since"),
    })
}

/// Serialise one record's verdict.
pub fn validity_to_json(set: &str, validity: &Validity) -> Json {
    let mut out = Json::obj();
    out.set("set", Json::Str(set.to_string()));
    out.set("state", Json::Str(validity.as_str().to_string()));
    out.set("detail", Json::Str(validity.detail()));
    out.set(
        "reason",
        match validity {
            Validity::Expired { reason, .. } => Json::Str((*reason).to_string()),
            _ => Json::Null,
        },
    );
    out.set(
        "checked",
        match validity {
            Validity::Valid { checked } => str_list(checked),
            _ => Json::Arr(alloc::vec::Vec::new()),
        },
    );
    out.set(
        "guards",
        str_list(
            &calibration::guards_fed_by(set)
                .iter()
                .map(|g| g.to_string())
                .collect::<alloc::vec::Vec<String>>(),
        ),
    );
    out
}

/// Serialise one starved guard (§45.5).
pub fn guard_unavailable_to_json(entry: &GuardUnavailable) -> Json {
    let mut out = Json::obj();
    out.set("guard", Json::Str(entry.guard.clone()));
    out.set("set", Json::Str(entry.set.clone()));
    out.set("state", Json::Str(entry.state.to_string()));
    out.set("detail", Json::Str(entry.detail.clone()));
    out
}

// ── Transfer: §12's fetch and §79's upload ───────────────────────────────────
//
// Two directions of one problem, and the decoders are deliberately not shared. `FetchSubject::size`
// is an `Option` because a release manifest may decline to declare one; `UploadSubject::size` is a
// `u64` because the window is a file this robot sealed. A single decoder would have to pick one,
// and picking either would put a shape on the other side that the contract does not have.

/// A bearer as either planner takes it (§34.3).
///
/// `metered` defaults to **true** and `metering_declared` to false, which is §34.3's cellular row
/// applied to silence: a case — or a robot — that has not said whether a link charges by the
/// megabyte has not established that it does not, and the expensive assumption is the safe one.
pub fn bearer_from_json(value: &Json) -> crate::bundle::Bearer {
    crate::bundle::Bearer {
        name: opt_string(value, "name").unwrap_or_default(),
        metered: value.get("metered").and_then(Json::as_bool).unwrap_or(true),
        metering_declared: flag(value, "metering_declared"),
    }
}

/// Read §12's fetch subject.
pub fn fetch_subject_from_json(value: &Json) -> crate::bundle::FetchSubject {
    crate::bundle::FetchSubject {
        digest: opt_string(value, "digest").unwrap_or_default(),
        size: byte_count(value, "size"),
        have: flag(value, "have"),
        partial: byte_count(value, "partial").unwrap_or(0),
        bearer: value.get_present("bearer").map(bearer_from_json),
    }
}

/// Read the bytes that are on the disk (§12).
pub fn arrived_from_json(value: &Json) -> crate::bundle::Arrived {
    crate::bundle::Arrived {
        digest: opt_string(value, "digest").unwrap_or_default(),
        size: byte_count(value, "size").unwrap_or(0),
    }
}

/// Read what a signed manifest says the bytes should be (§12).
pub fn expected_from_json(value: &Json) -> crate::bundle::Expected {
    crate::bundle::Expected {
        digest: opt_string(value, "digest").unwrap_or_default(),
        size: byte_count(value, "size"),
    }
}

/// Read §79's upload subject.
pub fn upload_subject_from_json(value: &Json) -> crate::upload::UploadSubject {
    crate::upload::UploadSubject {
        digest: opt_string(value, "digest").unwrap_or_default(),
        size: byte_count(value, "size").unwrap_or(0),
        // Absent and null are the same thing here and both mean *not asked this pass*, which is
        // the input that produces `probe`. An edition that read a missing `held` as zero would
        // report a receiver holding nothing, and send the window without asking.
        held: value.get_present("held").map(held_from_json),
        receipt: flag(value, "receipt"),
        bearer: value.get_present("bearer").map(bearer_from_json),
    }
}

/// Read what the receiver claims it holds (§79.2).
pub fn held_from_json(value: &Json) -> crate::upload::Held {
    crate::upload::Held {
        bytes: byte_count(value, "bytes").unwrap_or(0),
        digest: opt_string(value, "digest"),
    }
}

/// Read what the receiver reported after the last byte (§79.4).
pub fn reported_from_json(value: &Json) -> crate::upload::Reported {
    crate::upload::Reported {
        bytes: byte_count(value, "bytes").unwrap_or(0),
        digest: opt_string(value, "digest"),
    }
}

/// Read the window this robot sealed and measured (§79.4).
pub fn upload_window_from_json(value: &Json) -> crate::upload::Window {
    crate::upload::Window {
        digest: opt_string(value, "digest").unwrap_or_default(),
        size: byte_count(value, "size").unwrap_or(0),
    }
}

/// A whole, non-negative, representable byte count, or nothing.
///
/// The filter [`bundle_from_json`] spells inline, extracted because five decoders now need it and
/// four of them are about deleting or re-sending megabytes. A fractional or negative byte count is
/// not a smaller number — it is a producer that meant something else.
fn byte_count(value: &Json, key: &str) -> Option<u64> {
    value
        .get_present(key)
        .and_then(Json::as_f64)
        .filter(|value| *value >= 0.0 && *value <= u64::MAX as f64)
        .filter(|value| (*value as u64) as f64 == *value)
        .map(|value| value as u64)
}

// ── OTA (§12) ────────────────────────────────────────────────────────────────

/// Read a bundle manifest from a corpus case or a cached release document.
pub fn bundle_from_json(value: &Json) -> BundleManifest {
    BundleManifest {
        version: opt_string(value, "version").unwrap_or_default(),
        channel: opt_string(value, "channel").unwrap_or_default(),
        digest: opt_string(value, "digest").unwrap_or_default(),
        // Absent is not null and neither is zero: a manifest that declares no size has established
        // nothing, and a negative or fractional one is not a byte count. All three read as absent
        // rather than as a number no producer meant. See [`byte_count`].
        size: byte_count(value, "size"),
        compatible: value.str_arr("compatible"),
        changes: value.str_arr("changes"),
    }
}

/// Read the robot an update would be applied to.
///
/// The signature defaults to `unverifiable`, which is the one of §37.1's five states that is *about
/// the reader* rather than about the bundle — a case that did not say has not established anything,
/// and saying so is more honest than `absent`, which would be a claim that nobody signed the image.
pub fn update_subject_from_json(value: &Json) -> UpdateSubject {
    UpdateSubject {
        running_version: opt_string(value, "running_version").unwrap_or_default(),
        board: opt_string(value, "board"),
        channel: opt_string(value, "channel").unwrap_or_default(),
        signature: value
            .get("signature")
            .and_then(Json::as_str)
            .and_then(signature_state_from_str)
            .unwrap_or(SignatureState::Unverifiable),
        bench: value.get_present("bench").map(|bench| BenchPass {
            image_digest: opt_string(bench, "image_digest").unwrap_or_default(),
            tier: opt_string(bench, "tier").unwrap_or_default(),
            passed: flag(bench, "passed"),
        }),
    }
}

fn signature_state_from_str(text: &str) -> Option<SignatureState> {
    Some(match text {
        "verified" => SignatureState::Verified,
        "failed" => SignatureState::Failed,
        "untrusted" => SignatureState::Untrusted,
        "absent" => SignatureState::Absent,
        "unverifiable" => SignatureState::Unverifiable,
        _ => return None,
    })
}

/// Read §12's boot evidence.
///
/// Every health field decodes through [`Json::get_present`], so a case that omits one and a case
/// that writes `null` both arrive as "nobody has said" — which is the state `confirm_boot` is built
/// around and the one a `false` would silently replace with a failure.
pub fn boot_evidence_from_json(value: &Json) -> BootEvidence {
    let tristate = |key: &str| value.get_present(key).and_then(Json::as_bool);
    BootEvidence {
        pending: flag(value, "pending"),
        elapsed_ms: opt_f64(value, "elapsed_ms").unwrap_or(0.0),
        budget_ms: opt_f64(value, "budget_ms").unwrap_or(DEFAULT_BOOT_BUDGET_MS),
        post_passed: tristate("post_passed"),
        packages_healthy: tristate("packages_healthy"),
        converged: tristate("converged"),
    }
}

// ── The live precondition context (§54.2) ────────────────────────────────────

/// Read a calibration verdict back from a corpus case.
///
/// Only the **state** survives the round trip, and that is deliberate rather than lazy:
/// [`live::live_context`](crate::live::live_context) reads exactly two things from a [`Validity`] —
/// whether it is `Valid` and what it is called — and a decoder that reconstructed the sentence
/// would be inventing text nobody wrote. The one field that is not free-form, `Expired::reason`, is
/// looked up in [`EXPIRY_REASONS`](crate::calibration::EXPIRY_REASONS) so a case cannot introduce a
/// reason the vocabulary does not have.
fn validity_from_json(value: Option<&Json>) -> Validity {
    let state = value
        .and_then(|v| v.get("state"))
        .and_then(Json::as_str)
        .unwrap_or("absent");
    let detail = value
        .and_then(|v| v.get("detail"))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    match state {
        "valid" => Validity::Valid {
            checked: value.map(|v| v.str_arr("checked")).unwrap_or_default(),
        },
        "failed" => Validity::Failed { detail },
        "expired" => Validity::Expired {
            reason: value
                .and_then(|v| v.get("reason"))
                .and_then(Json::as_str)
                .and_then(|reason| {
                    calibration::EXPIRY_REASONS
                        .iter()
                        .copied()
                        .find(|known| *known == reason)
                })
                .unwrap_or("age"),
            detail,
        },
        "unusable" => Validity::Unusable { detail },
        _ => Validity::Absent,
    }
}

/// Read a base pose, component by component.
///
/// Deliberately **not** [`vec3`] and [`vec4`], and the difference is the whole reason this decoder
/// exists. Those two refuse the whole vector when one component is not a number, which would turn a
/// pose with one bad reading into a pose that was never reported — and those are exactly the two
/// facts [`live::live_context`](crate::live::live_context) has to tell apart: a robot with no base
/// pose omits the path silently, and a robot whose base pose has a NaN in it names `base_pose` in
/// [`LiveContext::unreadable`](crate::live::LiveContext::unreadable).
///
/// `null` decodes to `NaN` for the same reason it does for a joint: canonical JSON cannot carry a
/// non-finite number, so `null` is how a corpus case spells one, and both editions have to read it
/// as *"this component is not a reading"* rather than as *"this component is absent"*.
fn base_pose_from_json(pose: &Json) -> Option<BasePose> {
    fn component(items: &[Json], index: usize) -> f64 {
        items.get(index).and_then(Json::as_f64).unwrap_or(f64::NAN)
    }
    let xyz = pose.get("xyz")?.as_arr()?;
    let quaternion = pose.get("quaternion")?.as_arr()?;
    Some(BasePose {
        xyz: [component(xyz, 0), component(xyz, 1), component(xyz, 2)],
        quaternion: [
            component(quaternion, 0),
            component(quaternion, 1),
            component(quaternion, 2),
            component(quaternion, 3),
        ],
    })
}

fn joints_from_json(value: Option<&Json>) -> Vec<JointReading> {
    let mut readings = Vec::new();
    for entry in value.and_then(Json::as_arr).unwrap_or(&[]) {
        let Some(name) = entry.get("name").and_then(Json::as_str) else {
            continue;
        };
        readings.push(JointReading {
            name: name.to_string(),
            // A joint whose position is not a number at all is read as one that is not finite, and
            // the two reach §54.2 as the same fact: there is no reading. `NaN` is the value the
            // canonical form cannot carry, so a corpus case spells it `null` or omits it, and both
            // must land in [`LiveContext::unreadable`] rather than in the context.
            position: entry
                .get("position")
                .and_then(Json::as_f64)
                .unwrap_or(f64::NAN),
            // `None` and `Some(NaN)` reach the builder as the same fact — the body does not
            // measure it — and both omit the path. They are kept apart here only because the
            // canonical form cannot carry a `NaN`, so a case spells it `null`.
            velocity: opt_f64(entry, "velocity"),
            effort: opt_f64(entry, "effort"),
        });
    }
    readings
}

fn calibration_facts_from_json(value: Option<&Json>) -> Vec<CalibrationFact> {
    let mut facts = Vec::new();
    for entry in value.and_then(Json::as_arr).unwrap_or(&[]) {
        let Some(set) = entry.get("set").and_then(Json::as_str) else {
            continue;
        };
        facts.push(CalibrationFact {
            set: set.to_string(),
            validity: validity_from_json(entry.get_present("validity")),
            age_s: opt_f64(entry, "age_s"),
        });
    }
    facts
}

/// Read §54.2's inputs from a corpus case.
pub fn live_inputs_from_json(value: &Json) -> LiveInputs {
    LiveInputs {
        joints: joints_from_json(value.get_present("joints")),
        drive: value.get_present("drive").map(|drive| DriveState {
            estop: flag(drive, "estop"),
            degraded: flag(drive, "degraded"),
            braked: flag(drive, "braked"),
            homing: drive
                .get("homing")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            fault_code: opt_f64(drive, "fault_code"),
        }),
        gripper: opt_f64(value, "gripper"),
        battery_percent: opt_f64(value, "battery_percent"),
        base_pose: value.get_present("base_pose").and_then(base_pose_from_json),
        operating_state: value
            .get("operating_state")
            .and_then(Json::as_str)
            .and_then(OperatingState::parse),
        guards_active: value
            .get_present("guards_active")
            .map(|_| value.str_arr("guards_active")),
        guards_unavailable: value.str_arr("guards_unavailable"),
        calibration: calibration_facts_from_json(value.get_present("calibration")),
        frames_resolved: value.str_arr("frames_resolved"),
        frames_fault: value.get_present("frames_fault").map(|fault| FrameFault {
            code: fault
                .get("code")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string(),
            subjects: fault.str_arr("subjects"),
            detail: fault
                .get("detail")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        tier: opt_string(value, "tier"),
        grants: value.str_arr("grants"),
        env: value.get_present("env").cloned(),
    }
}

// ── Power and thermal (§36) ──────────────────────────────────────────────────

/// Serialise §36's whole answer for one cycle: the state, the reserve, the thermal ladder, the
/// faults, and whether the running policy is still inside its declared budget.
///
/// One document rather than four, for the reason §46's is one: they are one question — *what is
/// this robot's power situation right now* — and a console that had to join them would be the
/// second place §36.1's table was written down. The budget is in here rather than beside it because
/// the answer to "may this policy keep running" is exactly as much a power answer as the state is;
/// §36.3 puts them in the same paragraph.
///
/// The four booleans are derived rather than left to a consumer, and they are four rather than one
/// because §36.1's rows differ in more than severity: `low` withholds leases and lets a running
/// task finish, `reserve` asks for a safe state, `critical` asks for it immediately, and a robot
/// with a dead gauge withholds leases while being in none of those rows.
pub fn power_to_json(
    profile: &crate::power::PowerProfile,
    observation: &crate::power::PowerObservation,
    declared_budget_ms: Option<f64>,
    observed_inference_ms: Option<f64>,
) -> Json {
    let assessment = crate::power::assess(profile, observation);

    let mut reserve = Json::obj();
    reserve.set("wh", nullable_num(assessment.reserve.wh));
    // An array in §36.2's own order, not sorted: the terms are a sentence about which parts of a
    // computation happened, and a canonical object would alphabetise them into nonsense.
    reserve.set(
        "unaccounted",
        Json::Arr(
            assessment
                .reserve
                .unaccounted
                .iter()
                .map(|term| Json::Str(term.to_string()))
                .collect(),
        ),
    );
    reserve.set("complete", Json::Bool(assessment.reserve.complete()));

    let mut faults = Json::arr();
    for fault in &assessment.faults {
        let mut row = Json::obj();
        row.set("code", Json::Str(fault.code.as_str().to_string()));
        row.set("detail", Json::Str(fault.detail.clone()));
        faults.push(row);
    }

    let mut out = Json::obj();
    out.set("state", Json::Str(assessment.state.as_str().to_string()));
    out.set("reason", Json::Str(assessment.reason.as_str().to_string()));
    out.set("remaining_wh", nullable_num(assessment.remaining_wh));
    out.set("reserve", reserve);
    out.set(
        "thermal",
        Json::Str(assessment.thermal.as_str().to_string()),
    );
    out.set(
        "shed",
        Json::Arr(
            assessment
                .shed
                .iter()
                .map(|rung| Json::Str(rung.to_string()))
                .collect(),
        ),
    );
    out.set("faults", faults);
    out.set("degraded", Json::Bool(assessment.degraded()));
    out.set(
        "grants_new_leases",
        Json::Bool(assessment.grants_new_leases()),
    );
    out.set(
        "requires_safe_state",
        Json::Bool(assessment.requires_safe_state()),
    );
    out.set(
        "stops_immediately",
        Json::Bool(assessment.stops_immediately()),
    );
    // §36.1's third row, which this document carried for neither of the other two's reasons: it was
    // simply missed. `notifies_operator` was computed and tested in `power.rs` and read by nothing,
    // so a robot that owed an operator a phone call had no way to say so. Added 2026-09-16.
    out.set(
        "notifies_operator",
        Json::Bool(assessment.state.notifies_operator()),
    );
    out.set(
        "budget_missed",
        nullable_str(&crate::power::budget_missed(
            declared_budget_ms,
            observed_inference_ms,
        )),
    );
    out
}

// ── The watchdog chain (§46) ─────────────────────────────────────────────────

/// Serialise §46's whole answer for one body: the chain, the command-loss verdict, the nesting.
///
/// One document rather than three routes, because the three are one question — *is this robot's
/// watchdog chain sound* — and a console that had to join them would be the fourth place the
/// nesting rule was written down.
pub fn watchdog_to_json(on_command_loss: Option<&str>, windows: &crate::watchdog::Windows) -> Json {
    let mut layers = Json::arr();
    for layer in crate::watchdog::CHAIN {
        let mut row = Json::obj();
        row.set("layer", Json::Num(layer.layer as f64));
        row.set("name", Json::Str(layer.name.to_string()));
        row.set("catches", Json::Str(layer.catches.to_string()));
        row.set("response", Json::Str(layer.response.to_string()));
        row.set("required_from", Json::Str(layer.required_from.to_string()));
        row.set("petted_by", Json::Str(layer.petted_by.to_string()));
        layers.push(row);
    }

    let report = crate::watchdog::check_nesting(windows);
    let mut faults = Json::arr();
    for fault in &report.faults {
        let mut row = Json::obj();
        row.set("code", Json::Str(fault.code.to_string()));
        row.set("inner", Json::Str(fault.inner.to_string()));
        row.set("outer", Json::Str(fault.outer.to_string()));
        row.set("detail", Json::Str(fault.detail.clone()));
        faults.push(row);
    }

    // An **array**, not an object, and the reason is the whole subject of this document: these four
    // windows mean nothing except in order. A canonical JSON object sorts its keys, so the object
    // form renders them alphabetically — `drive_command_timeout_ms` first and
    // `kernel_cycle_worst_case_ms` third — and a panel about nesting that lists them out of nesting
    // order is worse than one that lists nothing. Found by looking at the page.
    let mut declared = Json::arr();
    for (name, value) in windows.ordered() {
        let mut row = Json::obj();
        row.set("window", Json::Str(name.to_string()));
        row.set("ms", nullable_num(value));
        declared.push(row);
    }

    let mut nesting = Json::obj();
    nesting.set("declared", declared);
    nesting.set("faults", faults);
    nesting.set(
        "missing",
        str_list(
            &report
                .missing
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<String>>(),
        ),
    );
    // Two booleans, because they are two different facts and a single one would have to lie about
    // one of them: nothing contradicts, and nothing was established.
    nesting.set("ordered", Json::Bool(report.ordered()));
    nesting.set("complete", Json::Bool(report.complete()));

    let mut out = Json::obj();
    out.set("chain", layers);
    out.set(
        "on_command_loss",
        nullable_str(&on_command_loss.map(ToString::to_string)),
    );
    out.set(
        "host_blocked",
        nullable_str(&crate::watchdog::host_blocked_by_command_loss(
            on_command_loss,
        )),
    );
    out.set("nesting", nesting);
    out
}

// ── §10's identity, pairing and attestation ──────────────────────────────────

/// Read the credential and key a robot holds.
///
/// `scopes` decodes through [`Json::get_present`] and stays `None` for both an absent key and an
/// explicit `null`, because that is the ordinary shape of an opaque bearer token — and it is a
/// different fact from an empty list, which is a credential that declares it may do nothing.
pub fn identity_from_json(value: &Json) -> crate::identity::Identity {
    crate::identity::Identity {
        device_id: opt_string(value, "device_id").unwrap_or_default(),
        namespace: opt_string(value, "namespace").unwrap_or_default(),
        device_key: opt_string(value, "device_key").unwrap_or_default(),
        binding: value
            .get_present("binding")
            .and_then(Json::as_str)
            .and_then(crate::identity::KeyBinding::parse),
        credential: flag(value, "credential"),
        credential_prefix: opt_string(value, "credential_prefix").unwrap_or_default(),
        scopes: value.get_present("scopes").map(|scopes| match scopes {
            Json::Arr(_) => scopes
                .as_arr()
                .map(|rows| {
                    rows.iter()
                        .filter_map(Json::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }),
    }
}

// ── §50's birth certificate ──────────────────────────────────────────────────

/// Read a certificate.
///
/// `signature` has no default, for the reason [`SignatureState`] has no `Default`: `Verified`
/// would be a lie and `Absent` would be a document that looks constructed and attests nothing. A
/// spelling this edition does not recognise decodes to `Unverifiable` — *this platform cannot
/// check it* — which is the only one of the five that is true of a state nobody here can read.
pub fn birth_certificate_from_json(value: &Json) -> crate::birth::BirthCertificate {
    crate::birth::BirthCertificate {
        device_key: opt_string(value, "device_key").unwrap_or_default(),
        model: opt_string(value, "model").unwrap_or_default(),
        serial: opt_string(value, "serial").unwrap_or_default(),
        descriptor_digest: opt_string(value, "descriptor_digest").unwrap_or_default(),
        hardware_revision: opt_string(value, "hardware_revision").unwrap_or_default(),
        manufactured: opt_string(value, "manufactured").unwrap_or_default(),
        calibration_digests: value.str_arr("calibration_digests"),
        claims_ownership: value.str_arr("claims_ownership"),
        signature: opt_string(value, "signature")
            .and_then(|text| SignatureState::parse(&text))
            .unwrap_or(SignatureState::Unverifiable),
    }
}

/// Read what the machine reports about itself.
///
/// Every field decodes through [`Json::get_present`], so an absent key and an explicit `null` are
/// both `None` — §7.2's absent input, which makes a comparison unavailable rather than passed.
pub fn birth_observed_from_json(value: &Json) -> crate::birth::Observed {
    crate::birth::Observed {
        device_key: value
            .get_present("device_key")
            .and_then(Json::as_str)
            .map(ToString::to_string),
        serial: value
            .get_present("serial")
            .and_then(Json::as_str)
            .map(ToString::to_string),
        descriptor_digest: value
            .get_present("descriptor_digest")
            .and_then(Json::as_str)
            .map(ToString::to_string),
        hardware_revision: value
            .get_present("hardware_revision")
            .and_then(Json::as_str)
            .map(ToString::to_string),
    }
}

/// Serialise what checking one certificate against one machine concluded.
///
/// `attests`, `fully_confirmed` and `unchecked` are derived rather than stored, and all three go on
/// the wire anyway: a consumer recomputing "does this attest anything" from a finding list is a
/// consumer with its own copy of `voids_the_certificate`, and the second copy is the one that will
/// be wrong the day a finding is added.
pub fn birth_to_json(attested: &crate::birth::Attested) -> Json {
    let mut out = Json::obj();
    out.set("attests", Json::Bool(attested.attests()));
    out.set(
        "findings",
        Json::Arr(
            attested
                .findings
                .iter()
                .map(|finding| {
                    let mut item = Json::obj();
                    item.set("code", Json::from(finding.code()));
                    item.set("message", Json::Str(crate::birth::explain(finding)));
                    item.set("subjects", str_list(&finding.subjects()));
                    item
                })
                .collect(),
        ),
    );
    out.set("fully_confirmed", Json::Bool(attested.fully_confirmed()));
    out.set(
        "unchecked",
        Json::Arr(
            attested
                .unchecked()
                .iter()
                .map(|field| Json::from(*field))
                .collect(),
        ),
    );
    out
}

// -- §56's provenance classes ------------------------------------------------

/// Read a run bundle.
///
/// `device_signature` has no default for [`birth_certificate_from_json`]'s reason, and an
/// unrecognised spelling decodes to `Unverifiable` for the same one. `simulator`, `device_known`,
/// `birth_attested` and `image_attested` all default to **false**, which is the direction every
/// default in this file points: a bundle that did not say has not established anything, and the
/// three booleans that would make a result `field` are exactly the ones nobody should reach by
/// omission.
///
/// `simulator` defaulting false is the one exception in spirit, and §52 already settled it: the
/// obligation to declare sits on the simulator driver, because a simulator presented as a robot is
/// the failure and a robot presented as a robot is not. Here the asymmetry is safe in both
/// directions — a bundle whose simulator flag is absent still needs the other three to reach
/// `field`.
pub fn run_bundle_from_json(value: &Json) -> crate::provenance::RunBundle {
    crate::provenance::RunBundle {
        benchmark_digest: opt_string(value, "benchmark_digest").unwrap_or_default(),
        policy_digest: opt_string(value, "policy_digest").unwrap_or_default(),
        embodiment_digest: opt_string(value, "embodiment_digest").unwrap_or_default(),
        os_image_digest: opt_string(value, "os_image_digest").unwrap_or_default(),
        seeds: value.str_arr("seeds"),
        blackbox_segment: value
            .get_present("blackbox_segment")
            .and_then(Json::as_str)
            .map(ToString::to_string),
        tier: opt_string(value, "tier").unwrap_or_default(),
        enforcement: opt_string(value, "enforcement").unwrap_or_default(),
        simulator: flag(value, "simulator"),
        device_signature: opt_string(value, "device_signature")
            .and_then(|text| SignatureState::parse(&text))
            .unwrap_or(SignatureState::Unverifiable),
        device_known: flag(value, "device_known"),
        birth_attested: flag(value, "birth_attested"),
        image_attested: flag(value, "image_attested"),
    }
}

/// Serialise a classification.
///
/// `rankable`, `ranked_separately` and `replayable` are derived and all three go on the wire, for
/// [`birth_to_json`]'s reason: a leaderboard recomputing "may this be ranked" from a class string
/// is a leaderboard with its own copy of §56.2, and the second copy is the one that will admit a
/// simulator to the field table the day a class is added.
pub fn provenance_to_json(classified: &crate::provenance::Classified) -> Json {
    let mut out = Json::obj();
    out.set("class", Json::from(classified.class.as_str()));
    out.set("rankable", Json::Bool(classified.class.rankable()));
    out.set(
        "ranked_separately",
        Json::Bool(classified.class.ranked_separately()),
    );
    out.set(
        "reasons",
        Json::Arr(
            classified
                .reasons
                .iter()
                .map(|reason| {
                    let mut item = Json::obj();
                    item.set("blocks_field", Json::Bool(reason.blocks_field()));
                    item.set("code", Json::from(reason.code()));
                    item.set("message", Json::Str(crate::provenance::explain(reason)));
                    item.set("subjects", str_list(&reason.subjects()));
                    item
                })
                .collect(),
        ),
    );
    out.set("replayable", Json::Bool(classified.replayable()));
    out
}

/// Serialise a §56.5 roll-up comparison.
///
/// The sentence travels beside the boolean rather than being left to the caller, because §56.5's
/// whole point is that a disagreement is a defect to investigate — and a bare `false` is the shape
/// of a thing somebody quietly widens a tolerance around.
pub fn scores_to_json(on_robot: f64, on_hub: f64) -> Json {
    let agree = crate::provenance::scores_agree(on_robot, on_hub);
    let mut out = Json::obj();
    out.set("agree", Json::Bool(agree));
    out.set(
        "detail",
        if agree {
            Json::Null
        } else {
            Json::Str(crate::provenance::explain_disagreement(on_robot, on_hub))
        },
    );
    out
}

/// Read what the robot observes about itself.
pub fn identity_observed_from_json(value: &Json) -> crate::identity::Observed {
    crate::identity::Observed {
        birth_key: opt_string(value, "birth_key").unwrap_or_default(),
        birth_certificate: flag(value, "birth_certificate"),
    }
}

/// Read one of §10's three digests.
///
/// An unreadable or unrecognised `state` decodes to [`MeasureState::Unmeasured`](crate::identity::MeasureState::Unmeasured)
/// rather than to `Known`, for the reason every default in this file points the same way: a case
/// that did not say has not established that anything was measured, and the state that claims a
/// measurement is the one nobody should reach by omission.
pub fn measure_from_json(value: Option<&Json>) -> crate::identity::Measure {
    let state = value
        .and_then(|v| v.get("state"))
        .and_then(Json::as_str)
        .and_then(crate::identity::MeasureState::parse)
        .unwrap_or(crate::identity::MeasureState::Unmeasured);
    crate::identity::Measure {
        state,
        digest: value
            .and_then(|v| opt_string(v, "digest"))
            .unwrap_or_default(),
        why: value.and_then(|v| opt_string(v, "why")).unwrap_or_default(),
    }
}

/// Read what a robot is running.
pub fn attestation_from_json(value: &Json) -> crate::identity::Attestation {
    crate::identity::Attestation {
        device_id: opt_string(value, "device_id").unwrap_or_default(),
        os_image: measure_from_json(value.get("os_image")),
        policy: measure_from_json(value.get("policy")),
        embodiment: measure_from_json(value.get("embodiment")),
    }
}

// ── §11 and §17's check-in ───────────────────────────────────────────────────

/// Read one aggregate health reading.
///
/// A field that is absent, `null`, or an object with neither key decodes to a [`Measured`] with
/// nothing in it, which [`summarize`](crate::checkin::summarize) reports as unmeasured. There is no
/// arm that produces a value without an age *by default*: §33.6 requires an age on every field, and
/// a decoder that supplied one would be the exact hole the rule exists to close.
pub fn measured_from_json(value: Option<&Json>) -> crate::checkin::Measured {
    let Some(node) = value.filter(|node| **node != Json::Null) else {
        return crate::checkin::Measured::none();
    };
    crate::checkin::Measured {
        value: opt_f64(node, "value"),
        // `as_f64` and then a range check, rather than `as_usize`, so that a negative age arrives
        // as a number this edition refuses rather than as an absence. A clock that went backwards
        // is a fact worth reporting as `undated`.
        age_ms: node
            .get_present("age_ms")
            .and_then(Json::as_f64)
            .filter(|age| age.is_finite() && *age >= 0.0)
            .map(|age| age as u64),
    }
}

/// Read what the reconciler has left to do.
pub fn progress_from_json(value: &Json) -> crate::checkin::Progress {
    let mut pending = Vec::new();
    for entry in value.arr_or_empty("pending") {
        pending.push(crate::checkin::Pending {
            stage: opt_string(entry, "stage").unwrap_or_default(),
            deferred: flag(entry, "deferred"),
            reason: opt_string(entry, "reason").unwrap_or_default(),
        });
    }
    crate::checkin::Progress {
        observed_generation: opt_f64(value, "observed_generation").unwrap_or(0.0) as u64,
        desired_generation: opt_f64(value, "desired_generation").unwrap_or(0.0) as u64,
        pending,
    }
}

/// Read what was observed about health.
///
/// An unrecognised readiness word decodes to [`Readiness::Unknown`](crate::checkin::Readiness::Unknown)
/// rather than to `Ready`, for the reason every default in this file points the same way: a case
/// that did not say has not established that a policy is producing, and `unknown` is the one of the
/// four that claims nothing.
pub fn health_input_from_json(value: &Json) -> crate::checkin::HealthInput {
    let mut packages = Vec::new();
    for entry in value.arr_or_empty("packages") {
        packages.push(crate::checkin::PackageHealth {
            id: opt_string(entry, "id").unwrap_or_default(),
            live: flag(entry, "live"),
            readiness: entry
                .get("readiness")
                .and_then(Json::as_str)
                .and_then(crate::checkin::Readiness::parse)
                .unwrap_or(crate::checkin::Readiness::Unknown),
            age_ms: opt_f64(entry, "age_ms").unwrap_or(0.0) as u64,
        });
    }
    crate::checkin::HealthInput {
        packages,
        generation_lag: measured_from_json(value.get("generation_lag")),
        guard_trip_rate: measured_from_json(value.get("guard_trip_rate")),
        storage_headroom: measured_from_json(value.get("storage_headroom")),
        battery: measured_from_json(value.get("battery")),
        temperature_c: measured_from_json(value.get("temperature_c")),
    }
}

/// Read everything a check-in is built from.
pub fn check_in_input_from_json(value: &Json) -> crate::checkin::CheckInInput {
    crate::checkin::CheckInInput {
        device_id: opt_string(value, "device_id").unwrap_or_default(),
        agent: opt_string(value, "agent").unwrap_or_default(),
        contracts: opt_string(value, "contracts").unwrap_or_default(),
        state: opt_string(value, "state").unwrap_or_default(),
        progress: value
            .get("progress")
            .map(progress_from_json)
            .unwrap_or_default(),
        health: value
            .get("health")
            .map(health_input_from_json)
            .unwrap_or_default(),
        attestation: attestation_from_json(value.get("attestation").unwrap_or(&Json::Null)),
        desired_digest: opt_string(value, "desired_digest").unwrap_or_default(),
        limits: value.get_present("limits").map(guard_report_from_json),
    }
}

/// The strings in one array field, dropping anything that is not one.
fn strings(value: &Json, key: &str) -> Vec<String> {
    value
        .arr_or_empty(key)
        .iter()
        .filter_map(|item| item.as_str().map(alloc::string::ToString::to_string))
        .collect()
}

/// Read §7.6's guard report.
///
/// Lenient in the direction every decoder in this file is: a field that is not there is absent, and
/// absent is never a zero. `enforced` is the one exception worth naming — it defaults to `false`,
/// which is the claim-least direction: a document that forgot to say whether its limits are
/// enforced is not a document asserting that they are.
pub fn guard_report_from_json(value: &Json) -> crate::checkin::GuardReport {
    let mut unavailable = Vec::new();
    for entry in value.arr_or_empty("unavailable") {
        unavailable.push(crate::checkin::UnavailableGuard {
            guard: opt_string(entry, "guard").unwrap_or_default(),
            missing: strings(entry, "missing"),
        });
    }
    let mut inert = Vec::new();
    for entry in value.arr_or_empty("inert") {
        inert.push(crate::checkin::InertGuard {
            guard: opt_string(entry, "guard").unwrap_or_default(),
            reading: opt_string(entry, "reading").unwrap_or_default(),
        });
    }
    let mut notices = Vec::new();
    for entry in value.arr_or_empty("notices") {
        notices.push(crate::checkin::Notice {
            code: opt_string(entry, "code").unwrap_or_default(),
            subjects: strings(entry, "subjects"),
        });
    }
    crate::checkin::GuardReport {
        resolved_at: opt_string(value, "resolved_at").unwrap_or_default(),
        enforcement: opt_string(value, "enforcement").unwrap_or_default(),
        enforced: value.get("enforced") == Some(&Json::Bool(true)),
        joint_velocity_max_rad_s: opt_f64(value, "joint_velocity_max_rad_s"),
        joint_torque_max_nm: opt_f64(value, "joint_torque_max_nm"),
        base_velocity_max_m_s: opt_f64(value, "base_velocity_max_m_s"),
        min_control_hz: opt_f64(value, "min_control_hz"),
        max_delta_per_cycle: opt_f64(value, "max_delta_per_cycle"),
        self_collision_margin_m: opt_f64(value, "self_collision_margin_m"),
        source: opt_string(value, "source"),
        active: strings(value, "active"),
        unavailable,
        inert,
        notices,
    }
}

// ── §11's pull ───────────────────────────────────────────────────────────────

/// Read a desired-state document the robot was handed.
///
/// `decoded` defaults to **false**, which is the direction every default in this file points: a
/// case that did not say has not established that the response was a desired state, and the value
/// that claims it did is the one nobody should reach by omission.
pub fn offer_from_json(value: &Json) -> crate::intent::Offer {
    crate::intent::Offer {
        decoded: flag(value, "decoded"),
        decode_error: opt_string(value, "decode_error").unwrap_or_default(),
        generation: opt_f64(value, "generation").unwrap_or(0.0) as u64,
        digest: opt_string(value, "digest").unwrap_or_default(),
        packages: opt_f64(value, "packages").unwrap_or(0.0) as usize,
    }
}

/// Read what the robot is reconciling against now.
pub fn held_intent_from_json(value: &Json) -> crate::intent::Held {
    crate::intent::Held {
        generation: opt_f64(value, "generation").unwrap_or(0.0) as u64,
        digest: opt_string(value, "digest").unwrap_or_default(),
        running: opt_f64(value, "running").unwrap_or(0.0) as usize,
    }
}

/// Read one §74 telemetry window.
///
/// A counter that is absent, `null` or not a number reads as **unmeasured**, which is the whole of
/// §74.3 on the decoding side: a reader that defaulted a missing count to zero would erase the
/// distinction the section is about before any function got to see it.
pub fn telemetry_window_from_json(value: &Json) -> crate::telemetry::Window {
    let mut window = crate::telemetry::Window {
        from_ms: opt_f64(value, "from_ms"),
        to_ms: opt_f64(value, "to_ms"),
        elapsed_ms: opt_f64(value, "elapsed_ms"),
        counters: alloc::vec::Vec::new(),
    };
    let counters = value.get("counters").cloned().unwrap_or_else(Json::obj);
    for name in crate::telemetry::TELEMETRY_COUNTERS {
        if let Some(count) = opt_f64(&counters, name) {
            window = window.with(name, Some(count));
        }
    }
    window
}

/// Read a set of windows.
pub fn telemetry_windows_from_json(value: &Json) -> alloc::vec::Vec<crate::telemetry::Window> {
    value
        .arr_or_empty("windows")
        .iter()
        .map(telemetry_window_from_json)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_text_matches_ecmascript_in_the_positional_band() {
        assert_eq!(number_text(6.0), "6");
        assert_eq!(number_text(0.5), "0.5");
        assert_eq!(number_text(500.0), "500");
        assert_eq!(number_text(-80.0), "-80");
        assert_eq!(number_text(0.0), "0");
        assert_eq!(number_text(1e-6), "0.000001");
    }

    #[test]
    fn number_text_switches_to_exponential_where_ecmascript_does() {
        assert_eq!(number_text(1e21), "1e+21");
        assert_eq!(number_text(1e-7), "1e-7");
        assert_eq!(number_text(1.5e-7), "1.5e-7");
        assert_eq!(number_text(-1e21), "-1e+21");
    }

    #[test]
    fn fixed_rounds_the_way_to_fixed_does() {
        assert_eq!(fixed0(44.4444), "44");
        assert_eq!(fixed0(44.5), "45");
        assert_eq!(fixed0(100.0), "100");
        assert_eq!(fixed0(0.0), "0");
        assert_eq!(fixed(99.9, 1), "99.9");
        assert_eq!(fixed(99.94999, 3), "99.950");
        assert_eq!(fixed(0.5, 3), "0.500");
        assert_eq!(fixed(99.9, 3), "99.900");
    }

    #[test]
    fn fixed_pads_small_values_with_a_leading_zero() {
        assert_eq!(fixed(0.05, 3), "0.050");
        assert_eq!(fixed(0.0004, 3), "0.000");
    }

    #[test]
    fn to_fixed_matches_the_reference_on_the_awkward_case() {
        // JS: (1.005).toFixed(2) === "1.00", because 1.005 is below its decimal spelling.
        assert_eq!(fixed(1.005, 2), "1.00");
    }
}
