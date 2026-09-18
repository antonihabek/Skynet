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
    /// How far behind the world this stream's timestamps are, in milliseconds (§25.1, §47.2).
    ///
    /// The observation assembler needs it to know "how far back a camera's *now* actually is": a
    /// stream stamped at acquisition still describes an instant earlier than the stamp, because
    /// exposure, readout and any in-pipeline processing all happened before the sample existed.
    /// [`crate::observation::assemble`] subtracts it, and a stream that has not declared it is
    /// assembled at its raw stamp with the omission **named in the frame** rather than assumed to
    /// be zero.
    ///
    /// `None` is the §7.2 rule applied consistently: absent means nobody measured it, not that it
    /// is zero. Zero is a claim — a sensor whose sample is available the instant the event happens
    /// — and it is a claim no imaging pipeline can make. `rhal.sensor.latency` in the bench suite
    /// is what turns the declaration into a measurement.
    pub latency_ms: Option<f64>,
    /// Whether this stream reports **person distance** to the `proximity` guard (§5, §7.1).
    ///
    /// The seventh input to [`crate::safety`]'s `resolve_limits` and the last one with no path.
    /// Its two neighbours are read off the descriptor — a sensor whose modality is `imu`, a base
    /// that is `legged` — and this one could not be, because §5's modalities say what a sensor
    /// *produces* and detection is what somebody does with the output. A camera is not a person
    /// detector; a camera plus a model somebody validated is.
    ///
    /// Three states, and the third is why this is `Option` rather than `bool`:
    ///
    /// | | |
    /// |---|---|
    /// | `None` | **undeclared** — nobody considered it. The guard reports unavailable (§7.2) |
    /// | `Some(false)` | **declared absent** — considered, and this stream does not do it |
    /// | `Some(true)` | this stream reports person distance |
    ///
    /// The first two read identically downstream and differently to a person: a descriptor full
    /// of `false` is a vendor who answered the question, and one with nothing is a vendor who has
    /// not been asked it yet.
    pub detects_people: Option<bool>,
    /// Whether this driver's `read_sensor` may be called for this sensor from a thread other than
    /// the one running `read_state` and `write_command`, while they run (§89.1).
    ///
    /// The driver's promise, decoded by `detects_people`'s rule: `Some(true)` only for the JSON
    /// boolean, so a string, a number or `null` reads as undeclared and the stream stays on the
    /// control thread under §89.2 — the direction a malformed declaration must fail in.
    /// [`crate::media::capture_placement`] is what reads it.
    pub concurrent_read: Option<bool>,
}

/// What the body will accept, and how fast.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlProfile {
    /// Every mode this body accepts on some joint — the **union**, not the intersection (§86.1).
    ///
    /// A body whose arm is position-only and whose wheels are velocity-only declares both, and
    /// which joint takes which is the joints' own `control_modes`. Before §86 this had to be the
    /// intersection, because a command carried one mode for the whole body, and such a body
    /// therefore declared nothing at all and could not be commanded.
    pub modes: Vec<String>,
    /// Whether this driver reads the per-joint `modes` array on a command (§86.1).
    ///
    /// A descriptor field rather than an ABI version, for §24's homing precedent: a driver written
    /// before §86 omits it, reads as exactly what it is, and keeps working. What it costs such a
    /// body is that a plan mixing modes across its joints has every channel outside the first mode
    /// blocked, because there is no way to deliver them in one command.
    pub per_joint_modes: Option<bool>,
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
    /// What kind of figures these are (§5.3): `declared` or `measured`. Read as declared,
    /// validated nowhere here — see `on_command_loss` in `json.rs` for why an unrecognised
    /// value is kept rather than dropped. Absent renders as `declared`.
    pub basis: Option<String>,
}

/// The hardware windows a body declares for §46.3's nesting.
///
/// Both optional, and an absent one is **not a body without a watchdog** — it is a body nobody has
/// measured one on. §46.3 wants the nesting *"reported at startup rather than discovered in the
/// field"*, and a report that silently skipped an undeclared window would be reporting on a chain
/// it had not established.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeclaredWindows {
    /// The SoC hardware watchdog's window, milliseconds. Layer 3 of §46.1.
    pub soc_wdt_ms: Option<f64>,
    /// The drive's own command timeout, past which it faults without being told.
    pub drive_command_timeout_ms: Option<f64>,
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
    /// Capability tokens this body **and its adapter** provide (§8).
    ///
    /// The set `admit_package` checks a package's `embodiment.capabilities` against. It lives here
    /// rather than anywhere else for two reasons: this descriptor is produced by the adapter, for
    /// the body, so "the robot plus its adapter" is precisely what it is in a position to answer —
    /// and §11's invariant is that no decision on a robot requires a round trip, which rules out
    /// asking the Hub at install time on a machine that has been offline for a week.
    ///
    /// **Empty is a claim, not an absence.** A body that declares no capabilities provides none, and
    /// a package needing one is refused. That is the safe direction and it is deliberately not
    /// nullable: `None` would mean "nobody said what this robot can do", and admission would then
    /// have to choose between refusing everything and admitting everything. Both are worse than a
    /// vendor being required to write the list down.
    pub capabilities: Vec<String>,
    /// Vendor extension tokens this body provides, beyond the canonical vocabulary.
    ///
    /// Separate from [`Self::capabilities`] for the reason §5.1 keeps roles extensible: a token
    /// nobody standardised must be expressible without being mistaken for one that was.
    pub extensions: Vec<String>,
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
    /// What the drives do when their command source stops (§46.2).
    ///
    /// One of [`COMMAND_LOSS_BEHAVIOURS`]. The reason it is on the descriptor at all is layer 3 of
    /// §46.1's watchdog chain: a hardware watchdog's recovery is *resetting the application
    /// processor*, and what a robot does while its processor is resetting is a property of the
    /// **hardware**, not of anything this operating system does. Some hold. Some engage brakes.
    /// Some fault and disable. **Some coast** — and on such a body the watchdog's own recovery path
    /// is a fall.
    ///
    /// `None` means nobody declared it, which is not `hold` and is not safe to treat as `hold`. It
    /// blocks the host tier for the same reason `coast` does: the recovery path has not been
    /// characterised, so nobody can say what taking it costs. It is a declaration a vendor makes and
    /// [`crate::certification`] measures — `safety.command_loss.behaviour` observes what the joint
    /// actually does — because the declaration is a claim and this platform does not award tiers for
    /// claims.
    pub on_command_loss: Option<String>,
    /// What each kinematic chain's drives do when commands stop (§87.3), in the document's order.
    ///
    /// §53 carried the gap from the moment Part V was written: *"a humanoid may brake on the legs
    /// and coast on the arms. The field is currently one value per body and probably needs to be
    /// per chain."* Keyed by §5.1's chain vocabulary, which already exists and is what made this
    /// the smallest of §43's four open items.
    ///
    /// **Declaring this and [`Self::on_command_loss`] is refused** — §84.2's rule about two action
    /// spaces, for the same reason — and a chain this body has that the map does not cover is
    /// refused rather than inheriting a body-wide value, because inheriting makes a partially
    /// declared humanoid look fully declared. `crate::watchdog::resolve_command_loss` is where
    /// both rules live.
    ///
    /// The inner `Option` is a key whose value is not a string: read as **uncovered**, which is
    /// the refusal a missing key gets, so an edition cannot invent a second answer for it.
    pub on_command_loss_by_chain: Option<Vec<(String, Option<String>)>>,
    /// The two watchdog windows only the hardware knows (§46.3).
    ///
    /// The other two in the nest come from elsewhere: `envelope.watchdog_ms` is on this descriptor
    /// already, and the kernel's worst-case cycle is the kernel's to measure. These two are facts
    /// about the board and the drive, which makes the descriptor the only place they can come from.
    pub watchdog: Option<DeclaredWindows>,
    /// What this body declares about power and heat (§36.2, §36.3, §36.4).
    ///
    /// `None` and a block whose every field is null are **the same body** to every consumer, which
    /// is unlike [`Self::frames`] and worth saying rather than leaving to be discovered: a power
    /// profile carries no list whose emptiness is a claim, so there is nothing for the two spellings
    /// to disagree about. It is an `Option` so that a descriptor written before Part IV can be told
    /// apart from a vendor who filled the block in with nulls — a distinction nothing acts on, and
    /// one a console can show an operator chasing a missing declaration.
    pub power: Option<crate::power::PowerProfile>,
    /// The frame graph (§44.3), or `None` where the generator could not express this body's tree.
    ///
    /// Absent, null and empty are three different bodies. Absent and null both arrive here as
    /// `None` — the decoder cannot tell a schema that predates the field from a generator that
    /// declined, and neither can a guard, so both leave the pose guards unavailable. An empty
    /// `nodes` list is a body claiming to have no frames at all, which [`validate_frames`] rejects.
    pub frames: Option<FrameGraph>,
    /// The named motions this body's controller accepts (§84.1).
    ///
    /// §59.2's machines are commanded by uploading a program, and before this field a descriptor
    /// could only describe one as joints nobody could command. Empty — which is also what an absent
    /// field reads as — is a body that is not commanded this way, so every descriptor written
    /// before §84 is the joint-commanded body it always was.
    pub motions: Vec<EmbodimentMotion>,
}

// ── Motions, §84 ─────────────────────────────────────────────────────────────

/// The canonical motions (§84.1), in the order the section lists them.
///
/// §5.1's argument one level up: a vendor's `move_l` and another's `LIN` are the same motion, and a
/// closed vocabulary is what lets a mechanism say so.
pub const CANONICAL_MOTIONS: [&str; 9] = [
    "move_joint",
    "move_linear",
    "move_circular",
    "grasp",
    "release",
    "home",
    "wait",
    "tool_change",
    "io_set",
];

/// The canonical motion a declaration names, or `None`.
///
/// Exact — no trimming and no case folding. Every normalisation the corpus has pinned is a place two
/// editions disagreed, and a token in a closed set has one spelling.
pub fn canonical_motion(value: Option<&str>) -> Option<&'static str> {
    let value = value?;
    CANONICAL_MOTIONS
        .iter()
        .copied()
        .find(|candidate| *candidate == value)
}

/// One parameter a body's motion takes.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionParameter {
    /// The controller's name for it.
    pub name: String,
    /// Its unit, where declared.
    pub unit: Option<String>,
    /// Whether the controller refuses the motion without it — `Some` only when the declaration was
    /// a boolean.
    ///
    /// **Anything but `Some(false)` is read as required**, which is the fail-closed direction: an
    /// undeclared requirement refuses a plan somebody can read, where the other reading ships a
    /// command a controller rejects on a moving robot. [`validate_motions`] names a declaration
    /// that is not a boolean, so the reading is never the only thing that knows.
    pub required: Option<bool>,
    /// The controller's lower bound. `None` is absent — nobody declared one, never "unbounded".
    /// A field that is present and not a number is `Some(NaN)`: the reference's type is a bare
    /// number, so a null in that position is a non-finite value JSON cannot spell.
    pub min: Option<f64>,
    /// The upper bound, read the same way.
    pub max: Option<f64>,
}

impl MotionParameter {
    /// Whether transport treats this parameter as one a policy must supply.
    pub fn is_required(&self) -> bool {
        self.required != Some(false)
    }
}

/// One motion a body's controller accepts (§84.1).
#[derive(Debug, Clone, PartialEq)]
pub struct EmbodimentMotion {
    /// The controller's own spelling. It is what a command carries.
    pub name: String,
    /// The canonical-motion slot as declared, when it was a string. A value of any other type
    /// declares nothing and reads as absent.
    pub motion: Option<String>,
    /// What it takes.
    pub parameters: Vec<MotionParameter>,
}

/// Why a body's motion list cannot be used, as the wire spells it.
pub const MOTION_FAULT_CODES: [&str; 6] = [
    "unnamed",
    "duplicate_motion",
    "unknown_motion",
    "duplicate_parameter",
    "required_undeclared",
    "bound_invalid",
];

/// Why a body's motion list cannot be used.
///
/// Carries what it is about, for §44.5's reason: *"the motion list is malformed"* is not a sentence
/// anybody can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionFault {
    /// One of [`MOTION_FAULT_CODES`].
    pub code: &'static str,
    /// What it is about, in the order that matters.
    pub subjects: Vec<String>,
    /// One sentence.
    pub detail: String,
}

/// Check a body's motion list against §84.1 — what `embodiment.descriptor.valid` measures of it.
///
/// The first fault in declaration order, or `None`. Transport does not call this: it reads an
/// unrecognised canonical motion as absent and a repeated name as first-wins, which are the
/// readings §5.1 gives roles. This is for the party who can still fix the file.
pub fn validate_motions(motions: &[EmbodimentMotion]) -> Option<MotionFault> {
    let mut names: Vec<&str> = Vec::new();
    for motion in motions {
        if motion.name.is_empty() {
            return Some(MotionFault {
                code: "unnamed",
                subjects: Vec::new(),
                detail: String::from(
                    "A motion has an empty name, and a name is what a command carries.",
                ),
            });
        }
        if names.contains(&motion.name.as_str()) {
            return Some(MotionFault {
                code: "duplicate_motion",
                subjects: alloc::vec![motion.name.clone()],
                detail: format!(
                    "Two motions are named {}, so a command naming it is ambiguous.",
                    motion.name
                ),
            });
        }
        names.push(&motion.name);

        if let Some(declared) = &motion.motion {
            if canonical_motion(Some(declared)).is_none() {
                return Some(MotionFault {
                    code: "unknown_motion",
                    subjects: alloc::vec![motion.name.clone(), declared.clone()],
                    detail: format!(
                        "Motion {} says it is {declared}, which is not a canonical motion. Omit the field for a motion that is none of them.",
                        motion.name
                    ),
                });
            }
        }

        let mut parameters: Vec<&str> = Vec::new();
        for parameter in &motion.parameters {
            if parameter.name.is_empty() {
                return Some(MotionFault {
                    code: "unnamed",
                    subjects: alloc::vec![motion.name.clone()],
                    detail: format!("Motion {} has a parameter with an empty name.", motion.name),
                });
            }
            let subject = format!("{}.{}", motion.name, parameter.name);
            if parameters.contains(&parameter.name.as_str()) {
                return Some(MotionFault {
                    code: "duplicate_parameter",
                    detail: format!("Motion {} lists {} twice.", motion.name, parameter.name),
                    subjects: alloc::vec![subject],
                });
            }
            parameters.push(&parameter.name);

            if parameter.required.is_none() {
                return Some(MotionFault {
                    code: "required_undeclared",
                    detail: format!(
                        "Parameter {subject} does not say whether it is required. It is read as required, which refuses plans that omit it; declare true or false."
                    ),
                    subjects: alloc::vec![subject],
                });
            }

            for (label, value) in [("min", parameter.min), ("max", parameter.max)] {
                if value.is_some_and(|value| !value.is_finite()) {
                    return Some(MotionFault {
                        code: "bound_invalid",
                        detail: format!(
                            "Parameter {subject} declares {label} as a value that is not a finite number."
                        ),
                        subjects: alloc::vec![subject],
                    });
                }
            }
            if let (Some(min), Some(max)) = (parameter.min, parameter.max) {
                if min > max {
                    return Some(MotionFault {
                        code: "bound_invalid",
                        detail: format!(
                            "Parameter {subject} declares a minimum above its maximum, so no value satisfies it."
                        ),
                        subjects: alloc::vec![subject],
                    });
                }
            }
        }
    }
    None
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
    /// `value = multiplier * driver + offset`. `None` means one.
    ///
    /// Added because the commonest linkage in robotics is a parallel gripper whose second finger
    /// runs at −1, and a field that could only say "follows" would put that finger on the wrong
    /// side of the object — inside the guard whose whole job is knowing where things are.
    pub mimic_multiplier: Option<f64>,
    /// Radians or metres, matching the joint kind. `None` means zero.
    pub mimic_offset: Option<f64>,
    /// Whether this frame carries collision geometry.
    pub collision: bool,
    /// The geometry, where it is a shape the Safety Kernel can check.
    ///
    /// `collision: true` with no shape is a real and common body: the vendor's model has meshes and
    /// nobody has reduced them to a primitive. It is not the same as `collision: false`, and it is
    /// not the same as a shape — the first says "there is geometry here that this kernel cannot
    /// evaluate", and the `self_collision` guard reports itself unavailable naming those frames
    /// rather than testing a body with limbs missing from it.
    pub collision_shape: Option<CollisionShape>,
    /// Frames this one may touch.
    ///
    /// Every real body has some. A humanoid's pelvis and torso overlap by design; URDF cannot say
    /// so and SRDF's `disable_collisions` exists for exactly this. Without it the guard trips on
    /// the robot standing still, and a guard that does that is one an operator switches off — the
    /// failure §7.1 spends a paragraph avoiding.
    pub collision_ignore: Vec<String>,
    /// Mass, where the geometry declares one.
    pub mass_kg: Option<f64>,
}

/// Collision geometry, as a sphere-swept segment.
///
/// One primitive, and it is the right one for a kernel that must finish inside a control cycle: a
/// capsule is a sphere when its endpoints coincide, it bounds a cylinder exactly and a link's real
/// geometry conservatively, and the distance between two of them is a closed form. Conservative is
/// the correct direction of error — the shape contains what it stands for, so the guard trips early
/// rather than late.
#[derive(Debug, Clone, PartialEq)]
pub struct CollisionShape {
    /// `capsule` today. The field exists so a body that is something else has to say so rather than
    /// be read as a capsule with strange numbers.
    pub kind: String,
    /// One end of the segment, in the frame's own coordinates.
    pub a: [f64; 3],
    /// The other end. Equal to `a` for a sphere.
    pub b: [f64; 3],
    /// Metres.
    pub radius_m: f64,
}

/// Shapes the kernel can evaluate.
pub const COLLISION_SHAPES: [&str; 1] = ["capsule"];

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
    /// `cycle`, `joint_mismatch`, `unknown_kind`, `unknown_shape`, `collision_mismatch`,
    /// `ignore_missing` — or, from [`validate_frame_bindings`], `dangling_joint`,
    /// `mimic_missing`, `mimic_self`, `mimic_coefficient`.
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
/// Does this body declare something that reports person distance? (§5, §7.1's `proximity`.)
///
/// The derivation `resolve_limits`'s seventh `available` input has been waiting for.
///
/// **Any one sensor is enough.** The question is about the robot, not about a stream, so a body
/// with a head camera that detects people and four that do not has a person detector. No quorum,
/// and no "the best of them": two detectors do not make the claim stronger and one that stopped
/// working does not make it weaker, which is what the bench and field suites are for.
///
/// **A declaration is honoured and not checked against the modality.** §5 puts the reason on the
/// field: only the vendor knows whether their depth stream is one a person is measured in.
/// Filtering by modality here would overrule the party that built the pipeline on the strength of
/// a vocabulary describing something else.
///
/// **What this does not establish** is that the guard will ever see a number. `proximity` acts on
/// `cycle.nearest_person_m`, and a declaration says a stream exists rather than that anything
/// delivers it — the same gap `available.imu` has always had, one step wider, because no producer
/// on this edition fills that field at all. §80 carries the question of whether an active guard
/// with no producer deserves a name of its own.
pub fn has_people_detection(descriptor: &EmbodimentDescriptor) -> bool {
    descriptor
        .sensors
        .iter()
        .any(|sensor| sensor.detects_people == Some(true))
}

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

    // The collision geometry. Checked here rather than by the guard that consumes it, for the same
    // reason as everything above: a shape nobody can read presents as a limb quietly missing from
    // the collision test, and an ignore list naming a frame that does not exist presents as a pair
    // that trips when the operator believed they had excluded it.
    for node in &graph.nodes {
        if let Some(shape) = &node.collision_shape {
            if !COLLISION_SHAPES.contains(&shape.kind.as_str()) {
                return Some(fault(
                    "unknown_shape",
                    &[&node.name],
                    format!(
                        "Frame {} declares collision geometry of kind {}, which this edition \
                         cannot evaluate.",
                        node.name, shape.kind
                    ),
                ));
            }
            if !(shape.radius_m.is_finite() && shape.radius_m > 0.0) {
                return Some(fault(
                    "unknown_shape",
                    &[&node.name],
                    format!(
                        "Frame {}'s collision geometry has radius {}. A capsule of no radius \
                         bounds nothing, and a guard built on it would report clearance it has \
                         not established.",
                        node.name, shape.radius_m
                    ),
                ));
            }
            if !node.collision {
                return Some(fault(
                    "collision_mismatch",
                    &[&node.name],
                    format!(
                        "Frame {} declares a collision shape and says it carries no collision \
                         geometry. One of the two is wrong and nothing here can tell which.",
                        node.name
                    ),
                ));
            }
        }
        for ignored in &node.collision_ignore {
            if !graph.nodes.iter().any(|other| &other.name == ignored) {
                return Some(fault(
                    "ignore_missing",
                    &[&node.name, ignored],
                    format!(
                        "Frame {} is permitted to touch {}, which is not one of the frames. An \
                         exception naming nothing excludes nothing.",
                        node.name, ignored
                    ),
                ));
            }
        }
    }

    None
}

/// Check a frame graph against the joint list of the body it describes.
///
/// [`validate_frames`] is deliberately graph-only: it is given a tree and asked whether the tree is
/// well formed, and it answers without ever seeing the descriptor the tree came out of. That leaves
/// one whole class of defect uncaught, because the two most important names in a dynamic frame —
/// `joint` and `mimics` — do not refer to anything in the graph. They refer to the joint list.
///
/// A name that does not resolve there presents far from its cause. The kernel binds channels by
/// joint name, finds nothing for that frame, and [`crate::safety::kinematics::Kinematics::solve`]
/// marks it unresolved — so both pose guards trip on it, on every cycle, for as long as the
/// descriptor says what it says. That is the correct behaviour and it is also useless: the robot
/// holds still and the reason is a spelling in a file nothing was checking.
///
/// §44.5 asks for better than that. A guard which cannot run is supposed to report so **when the
/// lease is granted**, naming the frame, and `statically_resolvable` is that mechanism — but it
/// reads the joint's *kind*, so it catches a limb hanging below a ball joint and cannot catch a
/// joint name that binds to nothing, because the joint list is not in the graph. This is what
/// closes that gap: it turns a permanent in-cycle trip into a refusal while somebody is still
/// looking at the file.
///
/// So this is the second half of §44.3, and it is what `embodiment.frames.complete` measures that
/// [`validate_frames`] cannot. Returns `None` when every name in the graph resolves.
///
/// It does **not** require that every declared joint have a frame. A partial tree is legitimate —
/// a gripper whose fingers nobody modelled is a body with joints and no frames for them — and
/// §44.5 already has the guard report unavailable naming the frame it wanted. "Complete" here means
/// every name the graph uses resolves, not that the graph covers the body.
pub fn validate_frame_bindings(graph: &FrameGraph, joints: &[&str]) -> Option<FrameFault> {
    let declared = |name: &str| joints.contains(&name);

    for node in &graph.nodes {
        if node.kind == "dynamic" {
            if let Some(joint) = &node.joint {
                if !declared(joint) {
                    return Some(fault(
                        "dangling_joint",
                        &[&node.name, joint],
                        format!(
                            "Frame {} moves with {}, which the body does not declare. Nothing \
                             binds it, so its pose never resolves and both pose guards trip on it \
                             every cycle.",
                            node.name, joint
                        ),
                    ));
                }
            }
        }

        let has_coefficient = node.mimic_multiplier.is_some() || node.mimic_offset.is_some();

        let Some(mimics) = &node.mimics else {
            if has_coefficient {
                return Some(fault(
                    "mimic_coefficient",
                    &[&node.name],
                    format!(
                        "Frame {} declares a mimic multiplier or offset and no `mimics`. A \
                         coefficient with nothing to multiply describes a linkage that does not \
                         exist.",
                        node.name
                    ),
                ));
            }
            continue;
        };

        if !declared(mimics) {
            return Some(fault(
                "mimic_missing",
                &[&node.name, mimics],
                format!(
                    "Frame {} follows {}, which the body does not declare. A linkage read \
                     from a joint nobody drives never resolves, and takes every frame below it \
                     with it.",
                    node.name, mimics
                ),
            ));
        }

        if node.joint.as_deref() == Some(mimics.as_str()) {
            return Some(fault(
                "mimic_self",
                &[&node.name, mimics],
                format!(
                    "Frame {} follows the joint it is driven by. `value = multiplier \u{d7} driver \
                     + offset` over one value is a fixed point, not a linkage.",
                    node.name
                ),
            ));
        }

        for (label, value) in [
            ("mimic_multiplier", node.mimic_multiplier),
            ("mimic_offset", node.mimic_offset),
        ] {
            if let Some(value) = value {
                if !value.is_finite() {
                    return Some(fault(
                        "mimic_coefficient",
                        &[&node.name],
                        format!(
                            "Frame {} declares {label} as a value that is not a finite number. A \
                             linkage computed from it produces a pose that is not one.",
                            node.name
                        ),
                    ));
                }
            }
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
    /// The named motions this policy emits (§84.2). Declaring these **and** action channels is
    /// refused: two architectures, not two spellings.
    pub motions: Vec<SourceMotion>,
}

/// One motion a policy emits (§84.2) — [`EmbodimentMotion`] seen from the other side.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMotion {
    /// The name the policy commands it by.
    pub name: String,
    /// The canonical motion it is, when declared as a string.
    pub motion: Option<String>,
    /// The parameters the policy **supplies**, by name.
    pub parameters: Vec<String>,
    /// Whether the policy still functions without it. Only an explicit `true` makes it optional.
    pub optional: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;

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

    fn body_with(sensors: Vec<EmbodimentSensor>) -> EmbodimentDescriptor {
        EmbodimentDescriptor {
            schema_version: 1,
            robot_slug: String::from("bench"),
            digest: None,
            verified: false,
            base: String::from("fixed"),
            dof: 0.0,
            joints: Vec::new(),
            end_effectors: Vec::new(),
            sensors,
            control: ControlProfile::default(),
            envelope: None,
            solvers: Vec::new(),
            capabilities: Vec::new(),
            extensions: Vec::new(),
            simulator: false,
            on_command_loss: None,
            on_command_loss_by_chain: None,
            watchdog: None,
            frames: None,
            power: None,
            motions: Vec::new(),
        }
    }

    fn sensor(name: &str, modality: &str, detects: Option<bool>) -> EmbodimentSensor {
        EmbodimentSensor {
            name: String::from(name),
            role: None,
            modality: String::from(modality),
            width: None,
            height: None,
            fps: None,
            frame: None,
            latency_ms: None,
            detects_people: detects,
            concurrent_read: None,
        }
    }

    /// §5. Undeclared and declared-absent are one fact downstream and two in the descriptor.
    #[test]
    fn nobody_asked_and_answered_no_read_the_same_way() {
        let unasked = body_with(alloc::vec![sensor("head", "rgb", None)]);
        let answered = body_with(alloc::vec![sensor("head", "rgb", Some(false))]);
        assert!(!has_people_detection(&unasked));
        assert!(!has_people_detection(&answered));
        // And they are still different descriptors, which is the whole reason `false` is worth
        // writing: one vendor answered the question and the other has not been asked it.
        assert_ne!(
            unasked.sensors[0].detects_people,
            answered.sensors[0].detects_people
        );
    }

    /// One is enough, and the modality is not consulted.
    #[test]
    fn one_declaring_sensor_answers_for_the_body() {
        let mixed = body_with(alloc::vec![
            sensor("head", "rgb", Some(false)),
            sensor("torso_depth", "depth", Some(true)),
            sensor("imu", "imu", None),
        ]);
        assert!(has_people_detection(&mixed));

        // Implausible and still honoured. The alternative is a list of modalities a detector may
        // live behind, which is a vocabulary, which needs the governance §30 leaves open — and a
        // wrong entry in it would disable a guard on a robot whose vendor declared one.
        let odd = body_with(alloc::vec![sensor("wrist_ft", "force_torque", Some(true))]);
        assert!(has_people_detection(&odd));
    }

    #[test]
    fn a_body_with_no_sensors_declares_nothing() {
        assert!(!has_people_detection(&body_with(Vec::new())));
    }

    #[test]
    fn the_vocabularies_are_the_ones_the_specification_lists() {
        assert_eq!(JOINT_TYPES.len(), 4);
        assert_eq!(CONTROL_MODES.len(), 5);
        assert_eq!(BASE_KINDS.len(), 6);
        assert_eq!(EMBODIMENT_MODALITIES.len(), 9);
        assert_eq!(ACTION_KINDS.len(), 7);
    }

    fn bare(name: &str, parent: Option<&str>, kind: &str) -> FrameNode {
        FrameNode {
            name: String::from(name),
            parent: parent.map(String::from),
            kind: String::from(kind),
            translation: parent.map(|_| [0.0, 0.0, 0.1]),
            rotation: parent.map(|_| [0.0, 0.0, 0.0, 1.0]),
            source: parent.map(|_| String::from("descriptor")),
            joint: None,
            joint_type: None,
            axis: None,
            mimics: None,
            mimic_multiplier: None,
            mimic_offset: None,
            collision: true,
            collision_shape: None,
            collision_ignore: Vec::new(),
            mass_kg: None,
        }
    }

    fn dynamic(name: &str, parent: &str, joint: &str) -> FrameNode {
        FrameNode {
            joint: Some(String::from(joint)),
            joint_type: Some(String::from("revolute")),
            axis: Some([0.0, 1.0, 0.0]),
            ..bare(name, Some(parent), "dynamic")
        }
    }

    fn rooted(nodes: &[FrameNode]) -> FrameGraph {
        let mut all = Vec::new();
        all.push(bare("base_link", None, "root"));
        all.extend(nodes.iter().cloned());
        FrameGraph {
            root: String::from("base_link"),
            convention: String::from("rep103"),
            generator: Some(String::from("urdf")),
            nodes: all,
        }
    }

    /// The branch `corpus/frames/bindings.json` cannot hold.
    ///
    /// The generator writes every case input through `JSON.stringify`, which renders infinity as
    /// `null`, so a frozen case would describe a different body from the one its expectation was
    /// computed from. And `skynet-json` refuses `1e999` at parse — "a literal so large it overflows
    /// to infinity is a corrupt corpus file, not a value" — so in this edition the document is
    /// rejected before the check runs, and the only way to reach it is the way a driver would:
    /// with a graph built in memory.
    ///
    /// It is kept because the TypeScript edition has no parser of its own. Whoever calls it arrives
    /// through `JSON.parse`, which answers `Infinity` rather than refusing, and this rule is the
    /// only thing between that and a pose computed from one.
    #[test]
    fn a_coefficient_that_is_not_a_number_is_refused_even_though_no_corpus_case_can_say_so() {
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let graph = rooted(&[
                dynamic("drive", "base_link", "hand.finger.flex"),
                FrameNode {
                    mimics: Some(String::from("hand.finger.flex")),
                    mimic_multiplier: Some(value),
                    ..dynamic("follower", "drive", "hand.finger.distal")
                },
            ]);
            let fault =
                validate_frame_bindings(&graph, &["hand.finger.flex", "hand.finger.distal"])
                    .expect("a coefficient that is not a number is a fault");
            assert_eq!(fault.code, "mimic_coefficient");
            assert_eq!(fault.subjects.as_slice(), &[String::from("follower")]);
        }
    }

    /// The same value, arriving the way a real descriptor would.
    ///
    /// Not a duplicate of the test above: it pins *which layer* refuses, and the answer is that the
    /// parser gets there first. If `skynet-json` ever starts admitting non-finite literals, this
    /// test fails and the one above becomes the load-bearing one — which is the order the two
    /// should be read in.
    #[test]
    fn a_descriptor_carrying_1e999_is_refused_by_the_parser_before_any_of_this_runs() {
        assert!(
            skynet_json::Json::parse(r#"{"mimic_multiplier":1e999}"#).is_err(),
            "a literal that overflows to infinity is a corrupt document, not a body"
        );
    }
}
