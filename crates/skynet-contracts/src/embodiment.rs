//! The Embodiment Descriptor — the body, made explicit and typed.
//!
//! A phone app does not care whether the screen is 6.1" or 6.7". A robot policy cares enormously
//! whether the arm has six joints or seven, whether joint 3 is rated for 80 Nm or 200 Nm, and
//! whether the thing it calls `left_shoulder_pitch` is the joint this robot calls `J2`. Android
//! could get away with an abstraction that hides the hardware. A robot OS cannot: **the body is
//! part of the API**, and pretending otherwise is how you destroy a wrist.
//!
//! These are the declarations. [`crate::transport`] is what does something with them.

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
