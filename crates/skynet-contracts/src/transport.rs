//! Embodiment Transport — the heart of the system.
//!
//! For each channel of the policy's action space, what does this body do with it?
//!
//! The industry's current answer is that a policy is trained against an implicit body and is then
//! hand-ported to every other body by an engineer reading two URDFs side by side. That does not
//! scale to a catalogue of thousands of policies and hundreds of robots, and it is the single
//! reason a model published for one humanoid is useless on the humanoid next to it.
//!
//! # Pure, and computed identically in two places
//!
//! [`resolve_transport`] has no I/O, no clock and no database. It runs **on the Hub**, so a
//! repository page can answer "does this run on my G1, and how well?" before anyone downloads
//! forty gigabytes; and **on the robot**, at install time, with no network at all. Both compute the
//! same function over the same declared facts, so neither is authoritative and they cannot
//! disagree.
//!
//! # What it deliberately does not do
//!
//! It does not *perform* the retargeting. Solving IK is the runtime's job, in the loop, in a
//! real-time context. This module decides **what is possible and under what ceilings**, and hands
//! the ceilings to the Safety Kernel — which is the only component that can enforce them. Deciding
//! is complex and runs once; enforcing is trivial and runs at 1 kHz. Separating them is what makes
//! both tractable.
//!
//! And it never invents a number. A body with no declared envelope produces a plan that says so. A
//! transport plan that quietly assumed a plausible torque rating would be a safety check that
//! lies, which is the worst artifact this system could produce.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::embodiment::*;
use crate::roles::{parse_role, ParsedRole};

/// How one action channel reaches this body.
///
/// The six values are ordered by how much they cost in fidelity, and the plan's verdict is derived
/// from the worst one present. Naming `drop` and `blocked` separately matters: dropping a channel
/// the publisher marked optional is a degradation the operator may accept, while blocking a
/// required one is a refusal, and collapsing them would turn a refusal into a shrug.
pub const TRANSPORT_METHODS: [&str; 6] = [
    "direct",
    "remap",
    "retarget",
    "synthesize",
    "drop",
    "blocked",
];

/// How one action channel reaches this body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMethod {
    /// The joint name matches exactly.
    Direct,
    /// Different wire name, same canonical role.
    Remap,
    /// Different kinematics — driven through task space by an IK solver.
    Retarget,
    /// No comparable structure, but the command's *meaning* transfers.
    Synthesize,
    /// No equivalent, and the publisher declared the channel optional.
    Drop,
    /// No equivalent, and the policy requires it.
    Blocked,
}

impl TransportMethod {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TransportMethod::Direct => "direct",
            TransportMethod::Remap => "remap",
            TransportMethod::Retarget => "retarget",
            TransportMethod::Synthesize => "synthesize",
            TransportMethod::Drop => "drop",
            TransportMethod::Blocked => "blocked",
        }
    }
}

/// One channel's binding.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelBinding {
    /// Position in the policy's output vector.
    pub source_index: f64,
    /// The role the channel declared, verbatim.
    pub source_role: Option<String>,
    /// How it reaches the body.
    pub method: TransportMethod,
    /// Target joint or actuator, when one was found.
    pub target: Option<String>,
    /// Multiplier applied to the source value before it reaches the target, so a policy trained on
    /// a stronger body does not command more than this one is rated for. 1 means no scaling.
    pub scale: f64,
    /// Hard ceiling for this channel in the target's units, handed to the Safety Kernel.
    pub limit: Option<f64>,
    /// English, for logs. A UI translates from `method` plus the subjects.
    pub reason: String,
}

/// How one observation stream reaches this body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationMethod {
    /// Name and modality match.
    Direct,
    /// Matched by role.
    Remap,
    /// Present but below the trained resolution or frame rate.
    Downscale,
    /// Not present on this body.
    Missing,
}

impl ObservationMethod {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ObservationMethod::Direct => "direct",
            ObservationMethod::Remap => "remap",
            ObservationMethod::Downscale => "downscale",
            ObservationMethod::Missing => "missing",
        }
    }
}

/// One observation binding.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationBinding {
    /// The stream the policy asked for.
    pub required: String,
    /// How it is satisfied.
    pub method: ObservationMethod,
    /// The sensor supplying it.
    pub target: Option<String>,
    /// English, for logs.
    pub reason: String,
}

/// Why the plan is warning about something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportWarningCode {
    /// The policy declares no action space at all.
    NoSourceEmbodiment,
    /// The body description is not signed by its vendor.
    UnverifiedEmbodiment,
    /// The body declares no safety envelope.
    NoTargetEnvelope,
    /// Training and target degrees of freedom differ.
    DofMismatch,
    /// The body does not accept the mode the policy emits.
    ControlModeUnsupported,
    /// The policy's rate exceeds what the body accepts, so the loop is clamped.
    RateUnsupported,
    /// The resolved loop is slower than the controller's watchdog window.
    RateBelowWatchdog,
    /// A channel is scaled down to stay inside a joint rating.
    ScaledDown,
    /// A channel cannot be scaled because one of the two figures is missing.
    UnscalableChannel,
    /// Task-space retargeting is needed and unavailable.
    RetargetWithoutSolver,
    /// Joints on this body that the policy does not drive.
    UnmappedRoles,
    /// A required observation stream has no equivalent.
    ObservationMissing,
    /// An observation stream is present but poorer than the trained one.
    ObservationDegraded,
    /// The body declares no command latency.
    LatencyUndeclared,
}

impl TransportWarningCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TransportWarningCode::NoSourceEmbodiment => "no_source_embodiment",
            TransportWarningCode::UnverifiedEmbodiment => "unverified_embodiment",
            TransportWarningCode::NoTargetEnvelope => "no_target_envelope",
            TransportWarningCode::DofMismatch => "dof_mismatch",
            TransportWarningCode::ControlModeUnsupported => "control_mode_unsupported",
            TransportWarningCode::RateUnsupported => "rate_unsupported",
            TransportWarningCode::RateBelowWatchdog => "rate_below_watchdog",
            TransportWarningCode::ScaledDown => "scaled_down",
            TransportWarningCode::UnscalableChannel => "unscalable_channel",
            TransportWarningCode::RetargetWithoutSolver => "retarget_without_solver",
            TransportWarningCode::UnmappedRoles => "unmapped_roles",
            TransportWarningCode::ObservationMissing => "observation_missing",
            TransportWarningCode::ObservationDegraded => "observation_degraded",
            TransportWarningCode::LatencyUndeclared => "latency_undeclared",
        }
    }
}

/// One thing the operator should know before running this.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportWarning {
    /// Why.
    pub code: TransportWarningCode,
    /// English, for logs.
    pub message: String,
    /// The specific things named.
    pub subjects: Option<Vec<String>>,
}

/// Verdicts, worst to best.
///
/// `Refused` is a real refusal: the OS will not install the policy against this body. The other
/// three all run, and the difference between them is what the operator is told they are getting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportVerdict {
    /// Every channel bound directly.
    Direct,
    /// At least one chain goes through task space, or a command was synthesised.
    Retargeted,
    /// At least one optional channel was dropped.
    Degraded,
    /// At least one required channel has no equivalent.
    Refused,
}

impl TransportVerdict {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TransportVerdict::Direct => "direct",
            TransportVerdict::Retargeted => "retargeted",
            TransportVerdict::Degraded => "degraded",
            TransportVerdict::Refused => "refused",
        }
    }
}

/// Ceilings the Safety Kernel enforces, derived from the envelope and the bindings.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportClamp {
    /// Whole-body joint velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Whole-body joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Base translation ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Slowest loop the controller's watchdog tolerates.
    pub min_control_hz: Option<f64>,
    /// Per-channel ceilings, keyed by target joint name.
    pub per_channel: Vec<(String, f64)>,
    /// Where the envelope figures came from. A limit with no provenance is a rumour.
    pub source: Option<String>,
}

/// What this policy will do on this body.
#[derive(Debug, Clone, PartialEq)]
pub struct TransportPlan {
    /// The worst binding in the plan.
    pub verdict: TransportVerdict,
    /// Which body.
    pub robot_slug: String,
    /// Training descriptor digest.
    pub source_digest: Option<String>,
    /// Target descriptor digest.
    pub target_digest: Option<String>,
    /// The mode the policy emits in.
    pub control_mode: String,
    /// Rate the loop will actually run at — the policy's, clamped to what the body accepts.
    pub control_hz: Option<f64>,
    /// Every channel, in the policy's output order.
    pub bindings: Vec<ChannelBinding>,
    /// Every observation stream.
    pub observations: Vec<ObservationBinding>,
    /// Ceilings for the kernel.
    pub clamp: Option<TransportClamp>,
    /// Everything the operator should know.
    pub warnings: Vec<TransportWarning>,
}

/// Knobs the caller turns.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveTransportOptions {
    /// Allow task-space retargeting when direct and role matching fail.
    ///
    /// Off by default: retargeting changes what the policy is doing, and that should be a decision
    /// somebody made rather than a fallback that happens quietly.
    pub allow_retarget: bool,
    /// Rate the caller intends to run at, when it differs from the policy's declared rate.
    pub requested_hz: Option<f64>,
}

const JOINT_KINDS: [&str; 3] = ["joint_position", "joint_velocity", "joint_torque"];

fn is_joint_kind(kind: &str) -> bool {
    JOINT_KINDS.contains(&kind)
}

/// Which joint rating bounds a channel of this kind.
fn rating_for(kind: &str, limits: &JointLimits) -> Option<f64> {
    match kind {
        "joint_velocity" => limits.velocity_max_rad_s,
        "joint_torque" => limits.effort_max_nm,
        "joint_position" => {
            // Position is bounded by travel rather than by a single magnitude; the ceiling handed
            // to the kernel is the larger absolute bound, and the kernel holds both ends.
            match (limits.position_max_rad, limits.position_min_rad) {
                (Some(max), Some(min)) => Some(max.abs().max(min.abs())),
                _ => None,
            }
        }
        _ => None,
    }
}

fn control_mode_for(kind: &str) -> Option<&'static str> {
    match kind {
        "joint_position" => Some("position"),
        "joint_velocity" => Some("velocity"),
        "joint_torque" => Some("torque"),
        "cartesian" => Some("cartesian"),
        _ => None,
    }
}

/// How far a channel must be scaled to stay inside a joint's rating.
///
/// Returns 1 when either number is missing, and a warning is raised separately. Guessing a ratio
/// from one known side would be inventing the other, which is precisely what this module refuses to
/// do anywhere else.
fn scale_for(peak: Option<f64>, rating: Option<f64>) -> f64 {
    match (peak, rating) {
        (Some(peak), Some(rating)) if peak > 0.0 => {
            let ratio = rating / peak;
            if ratio < 1.0 {
                ratio
            } else {
                1.0
            }
        }
        _ => 1.0,
    }
}

struct TargetIndex<'a> {
    by_name: Vec<(&'a str, &'a EmbodimentJoint)>,
    by_role: Vec<(String, &'a EmbodimentJoint)>,
    chains: BTreeSet<String>,
    roles: Vec<String>,
}

impl<'a> TargetIndex<'a> {
    fn joint_by_name(&self, name: &str) -> Option<&'a EmbodimentJoint> {
        self.by_name
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, joint)| *joint)
    }

    fn joint_by_role(&self, key: &str) -> Option<&'a EmbodimentJoint> {
        self.by_role
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, joint)| *joint)
    }

    fn has_role(&self, key: &str) -> bool {
        self.by_role.iter().any(|(candidate, _)| candidate == key)
    }
}

fn index_target(target: &EmbodimentDescriptor) -> TargetIndex<'_> {
    let mut by_name = Vec::with_capacity(target.joints.len());
    let mut by_role: Vec<(String, &EmbodimentJoint)> = Vec::new();
    let mut chains = BTreeSet::new();
    let mut roles = Vec::new();

    for joint in &target.joints {
        by_name.push((joint.name.as_str(), joint));
        let Some(parsed) = parse_role(joint.role.as_deref()) else {
            continue;
        };
        let key = parsed.key();
        // First declaration wins. A body declaring the same role twice is a descriptor bug, and
        // silently preferring the later one would make the mapping depend on array order.
        if !by_role.iter().any(|(existing, _)| *existing == key) {
            by_role.push((key.clone(), joint));
        }
        chains.insert(parsed.chain.clone());
        if !roles.contains(&key) {
            roles.push(key);
        }
    }

    TargetIndex {
        by_name,
        by_role,
        chains,
        roles,
    }
}

/// Resolve how a policy's action space reaches a body.
pub fn resolve_transport(
    source: &SourceEmbodiment,
    target: &EmbodimentDescriptor,
    options: ResolveTransportOptions,
) -> TransportPlan {
    let mut warnings: Vec<TransportWarning> = Vec::new();
    let index = index_target(target);
    let mut bindings: Vec<ChannelBinding> = Vec::new();

    if !target.verified {
        warnings.push(TransportWarning {
            code: TransportWarningCode::UnverifiedEmbodiment,
            message: "This body description is not signed by its vendor. Its joint ratings are unattested.".to_string(),
            subjects: Some(alloc::vec![target.robot_slug.clone()]),
        });
    }

    if source.action_channels.is_empty() {
        warnings.push(TransportWarning {
            code: TransportWarningCode::NoSourceEmbodiment,
            message: "The policy declares no action space, so nothing about its fit to this body can be checked.".to_string(),
            subjects: None,
        });
    }

    if target.envelope.is_none() {
        warnings.push(TransportWarning {
            code: TransportWarningCode::NoTargetEnvelope,
            message: "No safety envelope has been declared for this robot. Per-joint ratings are the only ceilings available.".to_string(),
            subjects: Some(alloc::vec![target.robot_slug.clone()]),
        });
    }

    if let Some(dof) = source.dof {
        if dof != target.dof {
            warnings.push(TransportWarning {
                code: TransportWarningCode::DofMismatch,
                message: format!(
                    "The policy was trained on a {} degree-of-freedom body; this one has {}.",
                    crate::json::number_text(dof),
                    crate::json::number_text(target.dof)
                ),
                subjects: None,
            });
        }
    }

    if !target.control.modes.contains(&source.control_mode) {
        warnings.push(TransportWarning {
            code: TransportWarningCode::ControlModeUnsupported,
            message: format!(
                "This body does not accept {} commands.",
                source.control_mode
            ),
            subjects: Some(alloc::vec![source.control_mode.clone()]),
        });
    }

    if target.control.command_latency_ms.is_none() {
        warnings.push(TransportWarning {
            code: TransportWarningCode::LatencyUndeclared,
            message: "This body declares no command latency, so loop stability at the policy rate cannot be predicted.".to_string(),
            subjects: None,
        });
    }

    // Chains that cannot be satisfied joint-by-joint.
    //
    // Retargeting is a property of a whole kinematic chain, never of one joint: you cannot send an
    // elbow through task space while its shoulder goes direct. So the chains that need it are
    // decided first, and every channel on such a chain is then marked together.
    let retarget_chains = decide_retarget_chains(source, &index, target, options, &mut warnings);

    for channel in &source.action_channels {
        bindings.push(bind_channel(
            channel,
            target,
            &index,
            &retarget_chains,
            &mut warnings,
        ));
    }

    let observations = bind_observations(&source.observations, target, &mut warnings);
    let control_hz = resolve_control_hz(source, target, options, &mut warnings);
    let clamp = build_clamp(target, &bindings);
    let verdict = derive_verdict(&bindings);

    let unmapped: Vec<String> = index
        .roles
        .iter()
        .filter(|role| {
            !bindings.iter().any(|binding| {
                binding
                    .source_role
                    .as_deref()
                    .and_then(|r| parse_role(Some(r)))
                    .map(|parsed| parsed.key() == **role)
                    .unwrap_or(false)
            })
        })
        .cloned()
        .collect();
    if !unmapped.is_empty() {
        warnings.push(TransportWarning {
            code: TransportWarningCode::UnmappedRoles,
            message: format!(
                "{} joint{} on this body are not driven by the policy and will be held by the controller.",
                unmapped.len(),
                if unmapped.len() == 1 { "" } else { "s" }
            ),
            subjects: Some(unmapped),
        });
    }

    TransportPlan {
        verdict,
        robot_slug: target.robot_slug.clone(),
        source_digest: source.digest.clone(),
        target_digest: target.digest.clone(),
        control_mode: source.control_mode.clone(),
        control_hz,
        bindings,
        observations,
        clamp,
        warnings,
    }
}

/// Which chains must go through task space, and whether this body can actually do that.
///
/// A chain qualifies when at least one of its joint channels has no direct or role match. It is
/// only *granted* when the caller asked for retargeting and the body has a solver — otherwise the
/// chain's channels fall through to `blocked`, which is the honest outcome. A plan that promised
/// retargeting on a robot with no IK solver would fail at the first command.
fn decide_retarget_chains(
    source: &SourceEmbodiment,
    index: &TargetIndex<'_>,
    target: &EmbodimentDescriptor,
    options: ResolveTransportOptions,
    warnings: &mut Vec<TransportWarning>,
) -> BTreeSet<String> {
    let mut needs: BTreeSet<String> = BTreeSet::new();

    for channel in &source.action_channels {
        if !is_joint_kind(&channel.kind) {
            continue;
        }
        if let Some(joint) = &channel.joint {
            if index.joint_by_name(joint).is_some() {
                continue;
            }
        }
        let Some(parsed) = parse_role(channel.role.as_deref()) else {
            continue;
        };
        if index.has_role(&parsed.key()) {
            continue;
        }
        // The chain exists on this body but this particular joint does not — a six-joint arm asked
        // to run a seven-joint policy. That is exactly the case task space can rescue.
        if index.chains.contains(&parsed.chain) {
            needs.insert(parsed.chain.clone());
        }
    }

    if needs.is_empty() {
        return needs;
    }

    let can_retarget = options.allow_retarget && !target.solvers.is_empty();
    if !can_retarget {
        warnings.push(TransportWarning {
            code: TransportWarningCode::RetargetWithoutSolver,
            message: if options.allow_retarget {
                "Task-space retargeting is required but this body has no inverse-kinematics solver installed.".to_string()
            } else {
                "Task-space retargeting would be required, and was not requested.".to_string()
            },
            subjects: Some(needs.iter().cloned().collect()),
        });
        return BTreeSet::new();
    }

    needs
}

fn bind_channel(
    channel: &ActionChannel,
    target: &EmbodimentDescriptor,
    index: &TargetIndex<'_>,
    retarget_chains: &BTreeSet<String>,
    warnings: &mut Vec<TransportWarning>,
) -> ChannelBinding {
    let parsed = parse_role(channel.role.as_deref());

    // Non-joint channels are matched against the body's own facilities rather than its joints.
    if !is_joint_kind(&channel.kind) {
        return bind_non_joint_channel(channel, target);
    }

    // 1. Exact wire name. Two vendors agreeing on a joint name is rare and unambiguous when it
    //    happens, so it outranks role matching.
    if let Some(name) = &channel.joint {
        if let Some(exact) = index.joint_by_name(name) {
            return finish_joint_binding(
                channel,
                exact,
                TransportMethod::Direct,
                "Joint name matches exactly.".to_string(),
                warnings,
            );
        }
    }

    // 2. Canonical role. This is the mechanism that makes bodies interoperable at all.
    if let Some(parsed) = &parsed {
        let key = parsed.key();
        if let Some(by_role) = index.joint_by_role(&key) {
            let reason = format!("Mapped by role {} to \"{}\".", key, by_role.name);
            return finish_joint_binding(
                channel,
                by_role,
                TransportMethod::Remap,
                reason,
                warnings,
            );
        }

        // 3. Task space, when the chain was granted it above.
        if retarget_chains.contains(&parsed.chain) {
            return ChannelBinding {
                source_index: channel.index,
                source_role: channel.role.clone(),
                method: TransportMethod::Retarget,
                target: None,
                scale: 1.0,
                limit: None,
                reason: format!(
                    "Driven through task space on the {} chain; this body has no {}.",
                    parsed.chain, key
                ),
            };
        }
    }

    // 4. Optional channels may be dropped; required ones are a refusal.
    if channel.optional {
        return ChannelBinding {
            source_index: channel.index,
            source_role: channel.role.clone(),
            method: TransportMethod::Drop,
            target: None,
            scale: 0.0,
            limit: None,
            reason: "No equivalent on this body, and the policy declares the channel optional."
                .to_string(),
        };
    }

    ChannelBinding {
        source_index: channel.index,
        source_role: channel.role.clone(),
        method: TransportMethod::Blocked,
        target: None,
        scale: 1.0,
        limit: None,
        reason: match &parsed {
            Some(parsed) => format!(
                "This body has no {} and the policy requires it.",
                parsed.key()
            ),
            None => "The channel declares no canonical role, so it can only match a joint of the same name, and none exists.".to_string(),
        },
    }
}

fn bind_non_joint_channel(
    channel: &ActionChannel,
    target: &EmbodimentDescriptor,
) -> ChannelBinding {
    let base = |method: TransportMethod,
                target_name: Option<String>,
                scale: f64,
                limit: Option<f64>,
                reason: String| ChannelBinding {
        source_index: channel.index,
        source_role: channel.role.clone(),
        method,
        target: target_name,
        scale,
        limit,
        reason,
    };

    if channel.kind == "gripper" {
        let effector = target
            .end_effectors
            .iter()
            .find(|e| e.kind == "gripper" || e.kind == "dexterous_hand");
        return match effector {
            None if channel.optional => base(
                TransportMethod::Drop,
                None,
                0.0,
                None,
                "This body has no gripper; the channel is optional.".to_string(),
            ),
            None => base(
                TransportMethod::Blocked,
                None,
                1.0,
                None,
                "This body has no gripper and the policy requires one.".to_string(),
            ),
            Some(effector) => base(
                TransportMethod::Remap,
                Some(effector.name.clone()),
                1.0,
                effector.max_grip_force_n,
                format!(
                    "Driven through the {} \"{}\".",
                    effector.kind, effector.name
                ),
            ),
        };
    }

    if channel.kind == "base_velocity" {
        if target.base == "fixed" {
            return if channel.optional {
                base(
                    TransportMethod::Drop,
                    None,
                    0.0,
                    None,
                    "This body has a fixed base; the channel is optional.".to_string(),
                )
            } else {
                base(
                    TransportMethod::Blocked,
                    None,
                    1.0,
                    None,
                    "The policy commands base motion and this body has a fixed base.".to_string(),
                )
            };
        }
        let limit = target
            .envelope
            .as_ref()
            .and_then(|e| e.base_velocity_max_m_s);
        // A wheeled base driven by a policy trained on a legged one is a synthesis, not a remap:
        // the command means the same thing, and nothing below the base interface is comparable.
        let method = if target.base == "legged" {
            TransportMethod::Remap
        } else {
            TransportMethod::Synthesize
        };
        return base(
            method,
            Some("base".to_string()),
            scale_for(channel.peak, limit),
            limit,
            format!("Base velocity applied to this {} base.", target.base),
        );
    }

    if channel.kind == "cartesian" {
        let effector = target.end_effectors.iter().find(|e| e.kind != "none");
        return match effector {
            _ if target.solvers.is_empty() || effector.is_none() => {
                if channel.optional {
                    base(
                        TransportMethod::Drop,
                        None,
                        0.0,
                        None,
                        "End-effector pose control is unavailable on this body; the channel is optional.".to_string(),
                    )
                } else {
                    base(
                        TransportMethod::Blocked,
                        None,
                        1.0,
                        None,
                        "End-effector pose control needs an end effector and a solver; this body has neither.".to_string(),
                    )
                }
            }
            Some(effector) => base(
                TransportMethod::Retarget,
                Some(effector.name.clone()),
                1.0,
                None,
                format!(
                    "Solved onto \"{}\" using {}.",
                    effector.name, target.solvers[0]
                ),
            ),
            None => unreachable!("the guard arm above covers the None case"),
        };
    }

    // `discrete` — a mode switch or a named action. It carries no physical units, so nothing here
    // can bound it; the package's capability grants are what gate it.
    base(
        TransportMethod::Direct,
        None,
        1.0,
        None,
        "Discrete command passed through; it commands no actuator directly.".to_string(),
    )
}

fn finish_joint_binding(
    channel: &ActionChannel,
    joint: &EmbodimentJoint,
    method: TransportMethod,
    reason: String,
    warnings: &mut Vec<TransportWarning>,
) -> ChannelBinding {
    let rating = rating_for(&channel.kind, &joint.limits);
    let scale = scale_for(channel.peak, rating);
    let mode = control_mode_for(&channel.kind);

    if let Some(mode) = mode {
        if !joint.control_modes.iter().any(|m| m == mode) {
            return ChannelBinding {
                source_index: channel.index,
                source_role: channel.role.clone(),
                method: TransportMethod::Blocked,
                target: Some(joint.name.clone()),
                scale: 1.0,
                limit: rating,
                reason: format!("\"{}\" does not accept {mode} commands.", joint.name),
            };
        }
    }

    if scale < 1.0 {
        warnings.push(TransportWarning {
            code: TransportWarningCode::ScaledDown,
            message: format!(
                "Commands to \"{}\" are scaled to {}% because the policy was trained on a stronger joint.",
                joint.name,
                crate::json::fixed0(scale * 100.0)
            ),
            subjects: Some(alloc::vec![joint.name.clone()]),
        });
    } else if channel.peak.is_some() && rating.is_none() {
        warnings.push(TransportWarning {
            code: TransportWarningCode::UnscalableChannel,
            message: format!(
                "\"{}\" declares no rating for this control mode, so the policy's commands cannot be scaled to it.",
                joint.name
            ),
            subjects: Some(alloc::vec![joint.name.clone()]),
        });
    }

    ChannelBinding {
        source_index: channel.index,
        source_role: channel.role.clone(),
        method,
        target: Some(joint.name.clone()),
        scale,
        limit: rating,
        reason,
    }
}

fn bind_observations(
    required: &[ObservationRequirement],
    target: &EmbodimentDescriptor,
    warnings: &mut Vec<TransportWarning>,
) -> Vec<ObservationBinding> {
    required
        .iter()
        .map(|requirement| {
            let exact = target
                .sensors
                .iter()
                .find(|s| s.name == requirement.name && s.modality == requirement.modality);
            let candidate = exact.or_else(|| match_sensor_by_role(requirement, target));

            let Some(candidate) = candidate else {
                if !requirement.optional {
                    warnings.push(TransportWarning {
                        code: TransportWarningCode::ObservationMissing,
                        message: format!(
                            "The policy reads a {} stream called \"{}\" and this body has none.",
                            requirement.modality, requirement.name
                        ),
                        subjects: Some(alloc::vec![requirement.name.clone()]),
                    });
                }
                return ObservationBinding {
                    required: requirement.name.clone(),
                    method: ObservationMethod::Missing,
                    target: None,
                    reason: if requirement.optional {
                        "Not present on this body; the policy declares it optional.".to_string()
                    } else {
                        "Not present on this body.".to_string()
                    },
                };
            };

            let below = |wanted: Option<f64>, have: Option<f64>| {
                wanted.is_some_and(|wanted| have.unwrap_or(0.0) < wanted)
            };
            let degraded = below(requirement.width, candidate.width)
                || below(requirement.height, candidate.height)
                || below(requirement.fps, candidate.fps);

            if degraded {
                warnings.push(TransportWarning {
                    code: TransportWarningCode::ObservationDegraded,
                    message: format!(
                        "\"{}\" is lower resolution or slower than the stream the policy was trained on.",
                        candidate.name
                    ),
                    subjects: Some(alloc::vec![candidate.name.clone()]),
                });
                return ObservationBinding {
                    required: requirement.name.clone(),
                    method: ObservationMethod::Downscale,
                    target: Some(candidate.name.clone()),
                    reason: "Present but below the trained resolution or frame rate; frames are upscaled.".to_string(),
                };
            }

            ObservationBinding {
                required: requirement.name.clone(),
                method: if exact.is_some() {
                    ObservationMethod::Direct
                } else {
                    ObservationMethod::Remap
                },
                target: Some(candidate.name.clone()),
                reason: if exact.is_some() {
                    "Sensor name and modality match.".to_string()
                } else {
                    format!("Matched by role to \"{}\".", candidate.name)
                },
            }
        })
        .collect()
}

fn match_sensor_by_role<'a>(
    requirement: &ObservationRequirement,
    target: &'a EmbodimentDescriptor,
) -> Option<&'a EmbodimentSensor> {
    let parsed: ParsedRole = parse_role(requirement.role.as_deref())?;
    let key = parsed.key();
    target.sensors.iter().find(|sensor| {
        sensor.modality == requirement.modality
            && parse_role(sensor.role.as_deref()).is_some_and(|role| role.key() == key)
    })
}

/// The rate the loop will actually run at.
///
/// Clamped down to what the body accepts, never up: running a policy faster than it was trained
/// changes its dynamics, and running it slower is a documented degradation rather than a silent
/// one. The watchdog check is the one that matters most — a loop slower than the controller's
/// timeout faults *while the robot is moving*, which is worse than refusing to start.
fn resolve_control_hz(
    source: &SourceEmbodiment,
    target: &EmbodimentDescriptor,
    options: ResolveTransportOptions,
    warnings: &mut Vec<TransportWarning>,
) -> Option<f64> {
    let requested = options.requested_hz.or(source.control_hz)?;

    let mut resolved = requested;
    if let Some(max) = target.control.rate_hz_max {
        if requested > max {
            resolved = max;
            warnings.push(TransportWarning {
                code: TransportWarningCode::RateUnsupported,
                message: format!(
                    "The policy runs at {} Hz; this body accepts at most {} Hz, so the loop is clamped.",
                    crate::json::number_text(requested),
                    crate::json::number_text(max)
                ),
                subjects: None,
            });
        }
    }

    if let Some(watchdog_ms) = target.envelope.as_ref().and_then(|e| e.watchdog_ms) {
        if watchdog_ms > 0.0 {
            let min_hz = 1000.0 / watchdog_ms;
            if resolved < min_hz {
                warnings.push(TransportWarning {
                    code: TransportWarningCode::RateBelowWatchdog,
                    message: format!(
                        "At {} Hz the controller's {} ms watchdog fires between commands.",
                        crate::json::number_text(resolved),
                        crate::json::number_text(watchdog_ms)
                    ),
                    subjects: None,
                });
            }
        }
    }

    Some(resolved)
}

fn build_clamp(
    target: &EmbodimentDescriptor,
    bindings: &[ChannelBinding],
) -> Option<TransportClamp> {
    let envelope = target.envelope.as_ref();
    let mut per_channel: Vec<(String, f64)> = Vec::new();

    for binding in bindings {
        if let (Some(name), Some(limit)) = (&binding.target, binding.limit) {
            match per_channel.iter_mut().find(|(key, _)| key == name) {
                // A later binding on the same target overwrites, matching an object assignment.
                Some(entry) => entry.1 = limit,
                None => per_channel.push((name.clone(), limit)),
            }
        }
    }

    if envelope.is_none() && per_channel.is_empty() {
        return None;
    }

    let watchdog_ms = envelope.and_then(|e| e.watchdog_ms);
    Some(TransportClamp {
        joint_velocity_max_rad_s: envelope.and_then(|e| e.joint_velocity_max_rad_s),
        joint_torque_max_nm: envelope.and_then(|e| e.joint_torque_max_nm),
        base_velocity_max_m_s: envelope.and_then(|e| e.base_velocity_max_m_s),
        min_control_hz: watchdog_ms.filter(|ms| *ms > 0.0).map(|ms| 1000.0 / ms),
        per_channel,
        source: envelope.and_then(|e| e.source.clone()),
    })
}

/// The plan's verdict is the worst binding in it.
///
/// One blocked required channel refuses the whole plan, because a policy missing an actuator it
/// depends on does not do most of its job — it does something nobody has characterised.
pub fn derive_verdict(bindings: &[ChannelBinding]) -> TransportVerdict {
    if bindings
        .iter()
        .any(|b| b.method == TransportMethod::Blocked)
    {
        return TransportVerdict::Refused;
    }
    if bindings.iter().any(|b| b.method == TransportMethod::Drop) {
        return TransportVerdict::Degraded;
    }
    if bindings
        .iter()
        .any(|b| b.method == TransportMethod::Retarget || b.method == TransportMethod::Synthesize)
    {
        return TransportVerdict::Retargeted;
    }
    TransportVerdict::Direct
}

/// Whether the OS will install a policy under this plan. Refusals are the only hard stop.
pub fn is_installable(plan: &TransportPlan) -> bool {
    plan.verdict != TransportVerdict::Refused
}
