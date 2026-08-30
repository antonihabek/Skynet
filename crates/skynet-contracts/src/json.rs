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
        min_os_version: opt_string(value, "min_os_version"),
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
