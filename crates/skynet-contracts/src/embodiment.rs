//! The Embodiment Descriptor — the body, made explicit and typed.
//!
//! A phone app does not care whether the screen is 6.1" or 6.7". A robot policy cares enormously
//! whether the arm has six joints or seven, whether joint 3 is rated for 80 Nm or 200 Nm, and
//! whether the thing it calls `left_shoulder_pitch` is the joint this robot calls `J2`. Android
//! could get away with an abstraction that hides the hardware. A robot OS cannot: **the body is
//! part of the API**, and pretending otherwise is how you destroy a wrist.
//!
//! These are the declarations. [`crate::transport`] is what does something with them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Joint kinds, as a URDF would name them.
pub const JOINT_TYPES: [&str; 4] = ["revolute", "prismatic", "continuous", "fixed"];

/// Control modes a body may accept.
pub const CONTROL_MODES: [&str; 5] = ["position", "velocity", "torque", "trajectory", "cartesian"];

/// Base kinds. Decides whether base velocity commands mean anything, and whether balance applies.
pub const BASE_KINDS: [&str; 6] = ["fixed", "wheeled", "tracked", "legged", "aerial", "rail"];

/// Sensing modalities a body may offer and a policy may require.
pub const EMBODIMENT_MODALITIES: [&str; 9] = [
    "rgb",
    "depth",
    "stereo",
    "point_cloud",
    "imu",
    "force_torque",
    "tactile",
    "audio",
    "joint_state",
];

/// What one joint is rated for.
///
/// Every field is optional and absent means **nobody declared it**. That is a different fact from
/// "unlimited", and the planner treats it as such: a channel whose target joint declares no rating
/// is reported `unscalable_channel` rather than being scaled against an invented figure.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JointLimits {
    /// Lower travel bound, radians (or metres, prismatic).
    pub position_min_rad: Option<f64>,
    /// Upper travel bound.
    pub position_max_rad: Option<f64>,
    /// Angular velocity ceiling.
    pub velocity_max_rad_s: Option<f64>,
    /// Torque ceiling.
    pub effort_max_nm: Option<f64>,
}

/// One actuated joint.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbodimentJoint {
    /// The name the vendor's driver actually uses on the wire. Never normalised away.
    pub name: String,
    /// Canonical role, or `None` when the vendor declared none.
    pub role: Option<String>,
    /// One of [`JOINT_TYPES`].
    pub joint_type: String,
    /// What it is rated for.
    pub limits: JointLimits,
    /// Modes this specific joint accepts. A wrist may be position-only on a torque-capable robot.
    pub control_modes: Vec<String>,
}

/// Something on the end of a chain.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbodimentEndEffector {
    /// Wire name.
    pub name: String,
    /// Canonical role, when declared.
    pub role: Option<String>,
    /// `gripper`, `dexterous_hand`, `suction`, `tool` or `none`.
    pub kind: String,
    /// Degrees of freedom in the effector itself.
    pub dof: f64,
    /// Grip force ceiling, when rated.
    pub max_grip_force_n: Option<f64>,
    /// Opening, when declared.
    pub max_opening_m: Option<f64>,
}

/// Something that senses.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbodimentSensor {
    /// Wire name.
    pub name: String,
    /// Canonical role, when declared.
    pub role: Option<String>,
    /// One of [`EMBODIMENT_MODALITIES`].
    pub modality: String,
    /// Pixels across, for imaging modalities.
    pub width: Option<f64>,
    /// Pixels down.
    pub height: Option<f64>,
    /// Frames per second.
    pub fps: Option<f64>,
    /// Frame the readings are expressed in, so extrinsics are resolvable (§44).
    pub frame: Option<String>,
}

/// What the body will accept, and how fast.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlProfile {
    /// Modes the body accepts at all.
    pub modes: Vec<String>,
    /// Fastest loop the body will run.
    pub rate_hz_max: Option<f64>,
    /// Slowest loop the body will run.
    pub rate_hz_min: Option<f64>,
    /// Command accepted to torque applied, round trip.
    ///
    /// This is the number that decides whether a policy trained at 50 Hz is stable on this body,
    /// and almost nobody publishes it. Requiring it costs a vendor one measurement and turns "it
    /// feels sluggish on our robot" into an arithmetic check performed before download (§5.2).
    pub command_latency_ms: Option<f64>,
}

/// The robot's declared safety envelope. Mirrors the Hub's shape and the kernel's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SafetyEnvelope {
    /// Per-joint angular velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Per-joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Whole-body translation ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Command timeout before the controller faults, in milliseconds.
    pub watchdog_ms: Option<f64>,
    /// Whether a hardware emergency stop is fitted.
    pub estop: Option<bool>,
    /// Where the figures came from. A limit with no provenance is a rumour.
    pub source: Option<String>,
}

/// The body.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbodimentDescriptor {
    /// Schema version. 1 today.
    pub schema_version: u32,
    /// Which robot this describes.
    pub robot_slug: String,
    /// sha256 over the canonicalised descriptor. Every transport plan pins it.
    pub digest: Option<String>,
    /// Whether a signature over `digest` verified against a key registered to the vendor's
    /// namespace.
    ///
    /// `false` means the body description is unattested — usable, and reported as such, because an
    /// unsigned descriptor is still better than the guesswork it replaces.
    pub verified: bool,
    /// One of [`BASE_KINDS`].
    pub base: String,
    /// Degrees of freedom.
    pub dof: f64,
    /// Every actuated joint.
    pub joints: Vec<EmbodimentJoint>,
    /// Every end effector.
    pub end_effectors: Vec<EmbodimentEndEffector>,
    /// Every sensor.
    pub sensors: Vec<EmbodimentSensor>,
    /// Control capabilities.
    pub control: ControlProfile,
    /// The envelope, or `None` when nobody has declared one — **not** "no limits".
    pub envelope: Option<SafetyEnvelope>,
    /// IK solvers resident on this robot. Empty means task-space retargeting is not available.
    pub solvers: Vec<String>,
    /// Whether this body is simulated (§35.3, §70).
    ///
    /// The one field on this descriptor whose absence means `false` rather than "nobody said". A
    /// real robot should not have to declare what it is not, and the obligation to declare sits on
    /// the simulator driver. Made optional it would produce "nobody said whether this robot is
    /// real", which is a question a physical machine's existence answers.
    ///
    /// It caps the certification award at guest — see [`crate::certification::award_certification`]
    /// — and is read from here rather than from the driver's declared ceiling, because a simulator
    /// driver written in C++ has no language ceiling to fall back on.
    pub simulator: bool,
    /// The frame graph (§44.3), or `None` where the generator could not express this body's tree.
    ///
    /// Absent, null and empty are three different bodies. Absent and null both arrive here as
    /// `None` — the decoder cannot tell a schema that predates the field from a generator that
    /// declined, and neither can a guard, so both leave the pose guards unavailable. An empty
    /// `nodes` list is a body claiming to have no frames at all, which [`validate_frames`] rejects.
    pub frames: Option<FrameGraph>,
}

// ── The frame graph, §44.3 ───────────────────────────────────────────────────

/// One frame.
///
/// A node carries **at most one joint**, which is what keeps forward kinematics a walk rather than
/// a solver. It is also a real restriction: URDF cannot express anything else, MJCF can, and a body
/// with two joints on one link therefore emits no graph at all rather than one with a hinge missing
/// from it.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameNode {
    /// Frame name, unique within the graph.
    pub name: String,
    /// `None` on the root, and on nothing else.
    pub parent: Option<String>,
    /// `root`, `static` or `dynamic`.
    pub kind: String,
    /// Metres, from the parent frame. `None` on the root, which has nothing to be relative to.
    pub translation: Option<[f64; 3]>,
    /// Ordered `xyzw` (§44.4). `None` on the root.
    pub rotation: Option<[f64; 4]>,
    /// `descriptor` for a vendor's nominal value, `calibration` for one measured on this machine.
    pub source: Option<String>,
    /// Present exactly when `kind` is `dynamic`.
    pub joint: Option<String>,
    /// `revolute`, `continuous`, `prismatic`, `ball` or `floating`.
    pub joint_type: Option<String>,
    /// The joint axis, in this frame, before the joint is applied.
    pub axis: Option<[f64; 3]>,
    /// The joint this one follows, where it is a linkage rather than a degree of freedom.
    pub mimics: Option<String>,
    /// Whether this frame carries collision geometry.
    pub collision: bool,
    /// Mass, where the geometry declares one.
    pub mass_kg: Option<f64>,
}

/// The tree.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameGraph {
    /// Names the node whose parent is `None`. `base_link` by convention (§44.4), not by rule.
    pub root: String,
    /// `rep103` today. The field exists so a body that is *not* REP-103 has to say so.
    pub convention: String,
    /// What produced the tree. Provenance, never a licence to read the other fields differently.
    pub generator: Option<String>,
    /// Every frame, in no required order — the tree is defined by `parent`, not by position.
    pub nodes: Vec<FrameNode>,
}

/// Why a frame graph cannot be used.
///
/// Carries the frames it is about rather than only a code, because §44.5 requires the pose guards
/// to report unavailable **with the missing frame named**. "The frame graph is malformed" is not a
/// sentence an operator can act on.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameFault {
    /// One of `empty`, `root_missing`, `several_roots`, `duplicate_name`, `parent_missing`,
    /// `cycle`, `joint_mismatch`, `unknown_kind`.
    pub code: String,
    /// The frames this is about, in the order they matter.
    pub subjects: Vec<String>,
    /// One sentence naming what is wrong.
    pub detail: String,
}

fn fault(code: &str, subjects: &[&str], detail: String) -> FrameFault {
    FrameFault {
        code: code.into(),
        subjects: subjects.iter().map(|s| String::from(*s)).collect(),
        detail,
    }
}

/// Frame kinds a node may declare.
pub const FRAME_KINDS: [&str; 3] = ["root", "static", "dynamic"];

/// Check a frame graph against the rules §44.3 pins.
///
/// Called before anything derives kinematics from it. Every fault here would otherwise present far
/// from its cause: a missing parent as a frame that silently resolves to identity, a cycle as a
/// hang in the middle of a control cycle, a duplicate name as a limb that moves when a different
/// limb is commanded.
///
/// Returns `None` when the graph is usable. It does **not** check that the graph reaches any
/// particular frame — "the tree does not reach the end effector" is a question about a specific
/// guard's needs, and §44.5 has that guard report it with the frame it wanted named.
pub fn validate_frames(graph: &FrameGraph) -> Option<FrameFault> {
    if graph.nodes.is_empty() {
        return Some(fault("empty", &[], "The graph declares no frames.".into()));
    }

    for (index, node) in graph.nodes.iter().enumerate() {
        if graph.nodes[..index]
            .iter()
            .any(|other| other.name == node.name)
        {
            return Some(fault(
                "duplicate_name",
                &[&node.name],
                format!(
                    "Two frames are named {}, so a parent reference is ambiguous.",
                    node.name
                ),
            ));
        }
    }

    let roots: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.parent.is_none())
        .map(|node| node.name.as_str())
        .collect();
    if roots.len() > 1 {
        return Some(fault(
            "several_roots",
            &roots,
            format!(
                "{} frames have no parent, so this is a forest and not a tree.",
                roots.len()
            ),
        ));
    }
    if !graph.nodes.iter().any(|node| node.name == graph.root) {
        return Some(fault(
            "root_missing",
            &[&graph.root],
            format!("The declared root {} is not one of the frames.", graph.root),
        ));
    }

    for node in &graph.nodes {
        if !FRAME_KINDS.contains(&node.kind.as_str()) {
            return Some(fault(
                "unknown_kind",
                &[&node.name],
                format!(
                    "Frame {} has kind {}, which is not root, static or dynamic.",
                    node.name, node.kind
                ),
            ));
        }
        if (node.kind == "dynamic") != node.joint.is_some() {
            return Some(fault(
                "joint_mismatch",
                &[&node.name],
                if node.kind == "dynamic" {
                    format!("Frame {} is dynamic and names no joint.", node.name)
                } else {
                    format!("Frame {} is {} and names a joint.", node.name, node.kind)
                },
            ));
        }
        if let Some(parent) = &node.parent {
            if !graph.nodes.iter().any(|other| &other.name == parent) {
                return Some(fault(
                    "parent_missing",
                    &[&node.name, parent],
                    format!(
                        "Frame {} names the parent {}, which is not in the graph.",
                        node.name, parent
                    ),
                ));
            }
        }
    }

    // Every frame reaches the root by following parents. The walk is bounded by the node count, so
    // a cycle is a walk that ran out of frames rather than one that ran forever.
    for node in &graph.nodes {
        let mut here = node;
        for _ in 0..=graph.nodes.len() {
            match &here.parent {
                None => break,
                Some(parent) => match graph.nodes.iter().find(|other| &other.name == parent) {
                    Some(next) => here = next,
                    None => break,
                },
            }
        }
        if here.parent.is_some() {
            return Some(fault(
                "cycle",
                &[&node.name],
                format!(
                    "Following parents from {} never reaches the root.",
                    node.name
                ),
            ));
        }
    }

    None
}

// ── The source side: what the policy was trained on ──────────────────────────

/// Kinds of channel a policy's action space may carry.
pub const ACTION_KINDS: [&str; 7] = [
    "joint_position",
    "joint_velocity",
    "joint_torque",
    "cartesian",
    "base_velocity",
    "gripper",
    "discrete",
];

/// One channel of a policy's action space.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionChannel {
    /// Position in the policy's output vector. Bindings are returned in this order.
    pub index: f64,
    /// One of [`ACTION_KINDS`].
    pub kind: String,
    /// Canonical role this channel drives, when the publisher declared one.
    pub role: Option<String>,
    /// Joint name on the training body, for exact-name matching.
    pub joint: Option<String>,
    /// Unit of `peak`.
    pub unit: Option<String>,
    /// Peak magnitude the channel reached in training, in `unit`.
    ///
    /// Feeds the clamp: a channel whose peak exceeds the target joint's rating is scaled, and the
    /// scaling is reported rather than applied silently.
    pub peak: Option<f64>,
    /// Whether the policy still functions with this channel dropped. Defaults to false.
    pub optional: bool,
}

/// One stream a policy reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRequirement {
    /// Stream name on the training body.
    pub name: String,
    /// One of [`EMBODIMENT_MODALITIES`].
    pub modality: String,
    /// Canonical role, for role matching when the name differs.
    pub role: Option<String>,
    /// Trained-on width.
    pub width: Option<f64>,
    /// Trained-on height.
    pub height: Option<f64>,
    /// Trained-on frame rate.
    pub fps: Option<f64>,
    /// Whether the policy runs without it.
    pub optional: bool,
}

/// The body a policy was trained on, and what it emits.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceEmbodiment {
    /// Training robot, when known.
    pub robot_slug: Option<String>,
    /// Training descriptor digest, when known.
    pub digest: Option<String>,
    /// Training degrees of freedom.
    pub dof: Option<f64>,
    /// The mode the policy emits in.
    pub control_mode: String,
    /// The rate it was trained at.
    pub control_hz: Option<f64>,
    /// The action space.
    pub action_channels: Vec<ActionChannel>,
    /// The observation space.
    pub observations: Vec<ObservationRequirement>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_rating_is_not_an_unlimited_one() {
        let limits = JointLimits::default();
        assert_eq!(limits.effort_max_nm, None);
        // The distinction the whole module rests on: `None` reads as "nobody declared it", and
        // every consumer is required to say so rather than substitute a plausible number.
        assert_ne!(limits.effort_max_nm, Some(f64::MAX));
    }

    #[test]
    fn an_absent_envelope_is_distinct_from_an_empty_one() {
        let nobody_looked: Option<SafetyEnvelope> = None;
        let looked_and_found_nothing = Some(SafetyEnvelope::default());
        assert_ne!(nobody_looked, looked_and_found_nothing);
    }

    #[test]
    fn the_vocabularies_are_the_ones_the_specification_lists() {
        assert_eq!(JOINT_TYPES.len(), 4);
        assert_eq!(CONTROL_MODES.len(), 5);
        assert_eq!(BASE_KINDS.len(), 6);
        assert_eq!(EMBODIMENT_MODALITIES.len(), 9);
        assert_eq!(ACTION_KINDS.len(), 7);
    }
}
