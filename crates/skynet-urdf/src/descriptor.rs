//! URDF → embodiment descriptor.
//!
//! §63.4 calls this the highest-leverage tool in Part VIII, "because the work is largely already
//! done and sitting in a file the robot already has". A URDF carries the joint list and types,
//! position, velocity and effort limits, the link tree and the collision geometry. That is most of
//! a descriptor.
//!
//! What it does not carry is the two things that matter most, and this module's whole character is
//! how it handles them:
//!
//! | Field | What happens |
//! |---|---|
//! | `joints[].limits.*` | Copied. **As design values**, and the provenance block says so. |
//! | `frames` | Built from the link tree, with joint axes, so the kernel can do its own FK (§44.2). |
//! | `dof` | Counted — actuators only, so a mimicked finger is not a degree of freedom. |
//! | `joints[].role` | **`null`.** Suggestions go to the report; a human accepts them explicitly. |
//! | `envelope` | **`null`.** Always. |
//! | `control.modes`, `joints[].control_modes` | Empty unless an operator asserts them. A URDF says nothing. |
//!
//! # Why the envelope is null, every time, with no flag to change it
//!
//! A URDF `effort` limit is a design value from a CAD model, not a datasheet safety rating.
//! Copying it into `envelope.joint_torque_max_nm` would auto-populate a *safety* limit from a
//! number nobody verified — which is precisely the "invent a plausible figure" failure the whole
//! design exists to prevent. So the generator emits `envelope: null`, the provenance block records
//! where sourced figures must come from, and the robot renders "no safety envelope declared" until
//! a person supplies one.
//!
//! The per-joint limits are a different case and §63.4 treats them as one: they are ratings for a
//! *channel*, they are what the transport planner scales against, and a plan built from them is
//! reported as built from them. They are still design values, and the descriptor says so where
//! anyone reading it will see it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use skynet_contracts::readiness::URDF_DRAFT_TOOL;
use skynet_json::Json;

use crate::model::{UrdfJoint, UrdfModel};
use crate::suggest::{suggest_roles, RoleSuggestion, Side};

/// Base kinds a descriptor may declare, from `skynet_contracts::embodiment::BASE_KINDS`.
pub const BASE_KINDS: [&str; 8] = [
    "fixed",
    "wheeled",
    "tracked",
    "legged",
    "aerial",
    "rail",
    "surface",
    "submerged",
];

/// Control modes an operator may declare, from `skynet_contracts::embodiment::CONTROL_MODES`.
pub const CONTROL_MODES: [&str; 5] = ["position", "velocity", "torque", "trajectory", "cartesian"];

/// What the caller supplies that the URDF cannot.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Overrides the slug derived from `<robot name>`.
    pub robot_slug: Option<String>,
    /// An operator's assertion about which side a single-limb robot's joints belong to.
    pub assume_side: Option<Side>,
    /// Write suggested roles into `joints[].role`.
    ///
    /// This is the "and a human confirms" step of §63.4, and it is a separate, explicit action
    /// precisely so that it is one. The report still lists every suggestion and how it was reached,
    /// so accepting them is auditable rather than invisible.
    pub accept_suggested_roles: bool,
    /// The base kind, when the operator knows it.
    pub base: Option<String>,
    /// Control modes the body accepts. Nothing in a URDF says.
    pub control_modes: Vec<String>,
    /// The file the URDF came from, for the provenance block.
    pub source_file: Option<String>,
    /// sha256 of that file's bytes.
    pub source_sha256: Option<String>,
    /// The version of the tool doing the importing.
    pub tool_version: String,
}

/// How much attention a line of the report wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLevel {
    /// A decision that was made and is worth seeing.
    Info,
    /// Something a person must supply before this descriptor is finished.
    Action,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Info or Action.
    pub level: NoteLevel,
    /// The sentence.
    pub message: String,
}

/// What the import concluded, for a human to read.
///
/// Separate from the descriptor because the descriptor is a machine artifact and this is an
/// argument about it. Printing the reasoning into JSON comments — which JSON does not have — is how
/// tools end up with a `_note` field nobody reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    /// The slug the descriptor claims.
    pub robot_slug: String,
    /// Degrees of freedom: actuators, not joints.
    pub dof: usize,
    /// Joints in the URDF that are not actuators, and why each was left out.
    pub excluded: Vec<(String, String)>,
    /// Frames emitted.
    pub frame_count: usize,
    /// One entry per actuator.
    pub suggestions: Vec<RoleSuggestion>,
    /// Whether suggestions were written into the descriptor.
    pub roles_accepted: bool,
    /// Everything a reader needs to know, in the order it is worth reading.
    pub notes: Vec<Note>,
}

impl ImportReport {
    /// Roles that were suggested at all.
    pub fn suggested_count(&self) -> usize {
        self.suggestions.iter().filter(|s| s.role.is_some()).count()
    }

    /// Actuators whose side an operator could resolve.
    pub fn needs_side_count(&self) -> usize {
        self.suggestions.iter().filter(|s| s.needs_side).count()
    }
}

/// The descriptor, and the argument for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// The embodiment descriptor, ready to canonicalise.
    pub descriptor: Json,
    /// What was decided along the way.
    pub report: ImportReport,
}

/// Why an import could not produce a descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    /// What is missing and how to supply it.
    pub message: String,
}

/// Build a descriptor from a parsed URDF.
pub fn build(model: &UrdfModel, options: &ImportOptions) -> Result<Import, ImportError> {
    let robot_slug = resolve_slug(model, options)?;
    let base = resolve_base(model, options)?;
    validate_control_modes(&options.control_modes)?;

    let suggestions = suggest_roles(model, options.assume_side);
    let mut notes: Vec<Note> = Vec::new();

    let mut joints = Json::arr();
    for (joint, suggestion) in model.actuators().zip(suggestions.iter()) {
        joints.push(joint_to_json(
            joint,
            suggestion,
            options.accept_suggested_roles,
            &options.control_modes,
        ));
    }

    let excluded: Vec<(String, String)> = model
        .joints
        .iter()
        .filter(|joint| !joint.is_actuator())
        .map(|joint| {
            let why = if let Some(driver) = &joint.mimic {
                format!(
                    "mimics `{driver}`, so it is a linkage rather than an actuator and commanding \
                     it separately would fight the joint that drives it"
                )
            } else {
                format!("is a `{}` joint, which nothing commands", joint.joint_type)
            };
            (joint.name.clone(), why)
        })
        .collect();

    let frames = build_frames(model);
    let frame_count = match frames.get("nodes") {
        Some(Json::Arr(nodes)) => nodes.len(),
        _ => 0,
    };

    let mut control = Json::obj();
    control.set(
        "modes",
        Json::Arr(
            options
                .control_modes
                .iter()
                .map(|mode| Json::Str(mode.clone()))
                .collect(),
        ),
    );

    let mut descriptor = Json::obj();
    descriptor
        .set("schema_version", Json::Num(1.0))
        .set("robot_slug", Json::Str(robot_slug.clone()))
        // Nulls rather than omissions, because a person reads this file and "digest: null" states
        // that nobody has digested and signed it yet. `get_present` reads both spellings as absent,
        // so nothing downstream can tell the difference.
        .set("digest", Json::Null)
        .set("verified", Json::Bool(false))
        .set("base", Json::Str(base.clone()))
        .set("dof", Json::Num(model.actuators().count() as f64))
        .set("joints", joints)
        .set("end_effectors", Json::arr())
        .set("sensors", Json::arr())
        .set("control", control)
        .set("envelope", Json::Null)
        .set("solvers", Json::arr())
        // A URDF describes a machine somebody built. §35.3 puts the obligation to declare on the
        // simulator driver, so this is `false` rather than absent for the same reason the nulls
        // above are nulls: a person reads this file.
        .set("simulator", Json::Bool(false))
        .set("frames", frames)
        .set("provenance", provenance(options));

    // ── The report, in the order it is worth reading ────────────────────────
    notes.push(Note {
        level: NoteLevel::Action,
        message:
            "`envelope` is null, and no flag changes that. A URDF effort limit is a design value \
             from a CAD model, not a datasheet safety rating. Sourced figures come from the \
             vendor's documentation or a risk assessment; until one is supplied this body reports \
             \"no safety envelope declared\" and the guards that need one report unavailable."
                .to_string(),
    });

    notes.push(Note {
        level: NoteLevel::Info,
        message: format!(
            "Per-joint limits were copied from the URDF as design values. {} of {} actuators \
             declare an effort limit and {} declare a velocity limit; the rest carry no rating, \
             and the transport planner reports a channel bound to one of them as unscalable rather \
             than scaling it against a guess.",
            model
                .actuators()
                .filter(|j| declared(j).effort.is_some())
                .count(),
            model.actuators().count(),
            model
                .actuators()
                .filter(|j| declared(j).velocity.is_some())
                .count(),
        ),
    });

    let sentinel: Vec<String> = (model.actuators())
        .filter_map(|joint| {
            let found = declared(joint).sentinels;
            (!found.is_empty()).then(|| format!("`{}` ({})", joint.name, found.join(", ")))
        })
        .collect();
    if !sentinel.is_empty() {
        notes.push(Note {
            level: NoteLevel::Action,
            message: format!(
                "{} joint(s) declare a figure no joint has — a travel past {TRAVEL_SENTINEL} rad, \
                 an effort past {EFFORT_SENTINEL} or a speed past {VELOCITY_SENTINEL}, or a \
                 rating of zero or less — which a URDF writes to mean unlimited or unknown. Each \
                 is left out rather than copied as a rating: {}. Supply the real figure by hand \
                 if the joint has one.",
                sentinel.len(),
                sentinel.join(", ")
            ),
        });
    }

    if options.control_modes.is_empty() {
        notes.push(Note {
            level: NoteLevel::Action,
            message: format!(
                "`control.modes` and every joint's `control_modes` are empty, because a URDF does \
                 not say what the body accepts. Until they are filled in a transport plan against \
                 this descriptor warns `control_mode_unsupported` and blocks every channel with \
                 \"this joint does not accept position commands\" — truthful, and useless. Re-run \
                 with --control-modes position,velocity (valid: {}); it sets the body profile and \
                 every joint together.",
                CONTROL_MODES.join(", ")
            ),
        });
    }

    let suggested = suggestions.iter().filter(|s| s.role.is_some()).count();
    let needs_side = suggestions.iter().filter(|s| s.needs_side).count();
    if options.accept_suggested_roles {
        notes.push(Note {
            level: NoteLevel::Info,
            message: format!(
                "{suggested} of {} roles were accepted from suggestion, at your instruction. Each \
                 one and how it was reached is listed below; a wrong role does not fail loudly, it \
                 binds a policy's channel to the wrong joint.",
                suggestions.len()
            ),
        });
    } else if suggested > 0 {
        notes.push(Note {
            level: NoteLevel::Action,
            message: format!(
                "{suggested} of {} joints have a suggested role and every `role` in the descriptor \
                 is null. URDF carries no semantics, so these are name heuristics and a human \
                 confirms them: review the list below and re-run with --accept-suggested-roles, or \
                 edit the roles by hand. A body with no roles is still usable — it just cannot \
                 participate in cross-body remapping.",
                suggestions.len()
            ),
        });
    }
    if needs_side > 0 {
        notes.push(Note {
            level: NoteLevel::Action,
            message: format!(
                "{needs_side} joints name a limb and not which side it is on, which is normal for a \
                 single-arm robot. --assume-side left|right resolves them, and the suggestion is \
                 marked inferred because you asserted it rather than the file stating it."
            ),
        });
    }

    // URDF has no notion of an end effector — a gripper is a joint and a link like any other — so
    // `end_effectors` is empty and stays empty. It matters because the transport planner routes a
    // `gripper` channel to an end effector rather than to a joint, so a hand that exists in the
    // kinematics and not in `end_effectors[]` blocks the one channel most policies emit.
    //
    // Detected from joint names rather than from the suggestions, because the suggestions need a
    // side and this advice is worth giving whether or not one was asserted.
    let hand_joints = model
        .actuators()
        .filter(|joint| {
            let name = joint.name.to_ascii_lowercase();
            ["gripper", "grip", "claw", "jaw", "finger", "thumb"]
                .iter()
                .any(|word| name.contains(word))
        })
        .count();
    if hand_joints > 0 {
        notes.push(Note {
            level: NoteLevel::Action,
            message: format!(
                "`end_effectors` is empty, because URDF has no way to say \"this is a gripper\" — a \
                 hand is a joint and a link like any other. {hand_joints} joints look like a hand, \
                 and a policy's `gripper` channel binds to an end effector rather than to a joint, \
                 so it will be refused until one is declared by hand with its kind, dof and grip \
                 force."
            ),
        });
    }

    if !excluded.is_empty() {
        notes.push(Note {
            level: NoteLevel::Info,
            message: format!(
                "{} of {} joints are not actuators and are absent from `joints[]`. They are still \
                 in the frame graph, because forward kinematics needs them.",
                excluded.len(),
                model.joints.len()
            ),
        });
    }

    notes.push(Note {
        level: NoteLevel::Info,
        message: format!(
            "`frames` carries {frame_count} frames rooted at `{}`, with each moving frame naming \
             the joint that drives it and its axis. That is what lets the Safety Kernel compute its \
             own forward kinematics rather than subscribing to a transform tree it cannot bound the \
             staleness of (§44.2).",
            model.root_link
        ),
    });

    if !model.links.iter().any(|link| link.has_collision) {
        notes.push(Note {
            level: NoteLevel::Info,
            message:
                "No link declares collision geometry, so the `self_collision` and `workspace` \
                 guards will report unavailable on this body rather than evaluating against a \
                 shape nobody supplied."
                    .to_string(),
        });
    }

    notes.push(Note {
        level: NoteLevel::Action,
        message:
            "`digest` is null and `verified` is false. The descriptor is digested and signed by the \
             vendor's key; until then a transport plan against this body carries the \
             `unverified_embodiment` warning, which is the truthful state and not an error."
                .to_string(),
    });

    Ok(Import {
        descriptor,
        report: ImportReport {
            robot_slug,
            dof: model.actuators().count(),
            excluded,
            frame_count,
            suggestions,
            roles_accepted: options.accept_suggested_roles,
            notes,
        },
    })
}

/// A rating a joint could have: above this and it is a stand-in for "unlimited".
///
/// `DRY-COVERAGE-PLAN.md` F168. Published URDFs write `effort="1e30"`, `lower="-1e16"` and
/// `effort="-1"` to mean *unlimited* or *unknown*, and this importer copied them as ratings — a
/// descriptor declaring a 10³⁰ N·m joint, and a kernel then holding a travel of ±10¹⁶ rad as though
/// it bounded something. No joint anyone builds is rated at a million newton-metres, turns at ten
/// thousand radians a second, or travels a thousand radians and stops, so a figure past those is
/// read as what it is: not declared. Zero and negative ratings are the same — a joint that can
/// exert no torque is not an actuator, and `-1` is a placeholder.
const EFFORT_SENTINEL: f64 = 1.0e6;
const VELOCITY_SENTINEL: f64 = 1.0e4;
const TRAVEL_SENTINEL: f64 = 1.0e3;

/// The limits a joint actually declares, with the sentinels removed and named.
struct Declared {
    lower: Option<f64>,
    upper: Option<f64>,
    effort: Option<f64>,
    velocity: Option<f64>,
    sentinels: Vec<&'static str>,
}

fn declared(joint: &UrdfJoint) -> Declared {
    let mut sentinels = Vec::new();
    let mut rating = |value: Option<f64>, ceiling: f64, what: &'static str| match value {
        Some(v) if v.is_finite() && v > 0.0 && v < ceiling => Some(v),
        Some(_) => {
            sentinels.push(what);
            None
        }
        None => None,
    };
    let effort = rating(joint.limit_effort, EFFORT_SENTINEL, "effort");
    let velocity = rating(joint.limit_velocity, VELOCITY_SENTINEL, "velocity");
    let beyond =
        |end: Option<f64>| end.is_some_and(|v| !v.is_finite() || v.abs() >= TRAVEL_SENTINEL);
    let (lower, upper) = if beyond(joint.limit_lower) || beyond(joint.limit_upper) {
        sentinels.push("travel");
        (None, None)
    } else {
        (joint.limit_lower, joint.limit_upper)
    };
    Declared {
        lower,
        upper,
        effort,
        velocity,
        sentinels,
    }
}

fn joint_to_json(
    joint: &UrdfJoint,
    suggestion: &RoleSuggestion,
    accept: bool,
    control_modes: &[String],
) -> Json {
    let mut limits = Json::obj();
    // `set_opt` throughout: a limit the URDF did not declare is *absent*, never zero. Absent means
    // "nobody declared it", and every consumer is required to say so rather than substitute. A
    // sentinel is not a declaration either (`declared`).
    let limit = declared(joint);
    limits
        .set_opt("position_min_rad", limit.lower.map(Json::Num))
        .set_opt("position_max_rad", limit.upper.map(Json::Num))
        .set_opt("velocity_max_rad_s", limit.velocity.map(Json::Num))
        .set_opt("effort_max_nm", limit.effort.map(Json::Num));

    let role = match (accept, &suggestion.role) {
        (true, Some(role)) => Json::Str(role.clone()),
        _ => Json::Null,
    };

    let mut out = Json::obj();
    out.set("name", Json::Str(joint.name.clone()))
        .set("role", role)
        .set("type", Json::Str(joint.joint_type.clone()))
        .set("limits", limits)
        // A URDF says nothing about which modes a *specific* joint accepts, so an empty list is the
        // truthful answer — and the transport planner blocks every channel bound here until
        // somebody fills it in. An operator passing --control-modes is asserting what the body
        // accepts, and that assertion covers every joint: a wrist that is position-only on a
        // torque-capable robot is an exception that lives in the vendor's documentation rather than
        // in the kinematics, and is edited in by hand afterwards.
        .set(
            "control_modes",
            Json::Arr(
                control_modes
                    .iter()
                    .map(|mode| Json::Str(mode.clone()))
                    .collect(),
            ),
        );
    out
}

/// The frame graph (§44.3), built from the link tree.
///
/// Every link becomes a frame. A link reached through a movable joint is `dynamic` and names the
/// joint and axis that drive it; everything else is `static`. Both carry the joint origin, because
/// a dynamic frame's transform is the origin *composed with* the joint's motion, and a consumer
/// with only the joint name would have to go back to the URDF for the half that does not move.
fn build_frames(model: &UrdfModel) -> Json {
    let mut nodes = Json::arr();
    for link in model.links_from_root() {
        let mut node = Json::obj();
        node.set("name", Json::Str(link.name.clone()));

        match model.joint_to(&link.name) {
            None => {
                node.set("parent", Json::Null)
                    .set("kind", Json::Str("root".to_string()));
            }
            Some(joint) => {
                node.set("parent", Json::Str(joint.parent.clone()));
                let dynamic = joint.is_movable();
                node.set(
                    "kind",
                    Json::Str(if dynamic { "dynamic" } else { "static" }.to_string()),
                );
                if dynamic {
                    node.set("joint", Json::Str(joint.name.clone()))
                        .set("joint_type", Json::Str(joint.joint_type.clone()))
                        .set("axis", vector(&joint.axis));
                    if let Some(driver) = &joint.mimic {
                        node.set("mimics", Json::Str(driver.clone()));
                        // Written only when the URDF said so. §44.3's defaults are 1 and 0, and
                        // emitting them explicitly would make a file that declared nothing
                        // indistinguishable from one that declared the defaults — the same
                        // absent-versus-null distinction the whole descriptor turns on.
                        node.set_opt("mimic_multiplier", joint.mimic_multiplier.map(Json::Num))
                            .set_opt("mimic_offset", joint.mimic_offset.map(Json::Num));
                    }
                }
                node.set("translation", vector(&joint.origin_xyz))
                    .set("rotation", quaternion(joint.origin_rpy))
                    // §44.3: a static transform is either the vendor's nominal design value or one
                    // measured on this individual machine, and the frame records which. A URDF is
                    // nominal by definition; calibration writes the other value later (§45).
                    .set("source", Json::Str("descriptor".to_string()));
            }
        }

        node.set("collision", Json::Bool(link.has_collision));
        // Present only where the geometry reduced to a shape the kernel evaluates. A link with a
        // mesh keeps `collision: true` and gets no shape, which is §44.3's third state and the one
        // that makes the `self_collision` guard report unavailable rather than test a body with
        // limbs missing from it.
        if let Some(capsule) = link.collision {
            let mut shape = Json::obj();
            shape
                .set("kind", Json::Str("capsule".to_string()))
                .set("a", vector(&capsule.a))
                .set("b", vector(&capsule.b))
                .set("radius_m", Json::Num(capsule.radius_m));
            node.set("collision_shape", shape);
        }
        node.set_opt("mass_kg", link.mass_kg.map(Json::Num));
        nodes.push(node);
    }

    let mut frames = Json::obj();
    frames
        .set("root", Json::Str(model.root_link.clone()))
        // §44.4, stated in the artifact rather than only in the specification, because a sign error
        // here moves a robot the wrong way and the file is what a driver author reads.
        .set("convention", Json::Str("rep103".to_string()))
        // `generator`, not `source`: §44.3 gives `source` to the per-node descriptor-versus-
        // calibration distinction, and one name meaning two things in one object is how a consumer
        // reads the wrong one.
        .set("generator", Json::Str("urdf".to_string()))
        .set("nodes", nodes);
    frames
}

fn vector(values: &[f64; 3]) -> Json {
    Json::Arr(values.iter().copied().map(Json::Num).collect())
}

/// URDF fixed-axis roll-pitch-yaw to a quaternion, ordered `xyzw` per §44.4.
///
/// URDF's `rpy` is extrinsic X then Y then Z, which composes as `Rz(yaw) · Ry(pitch) · Rx(roll)`.
/// Getting this order wrong produces a frame graph that is subtly, consistently wrong — the kind of
/// error that looks like a calibration problem for a week.
fn quaternion(rpy: [f64; 3]) -> Json {
    let [roll, pitch, yaw] = rpy;
    let (sr, cr) = ((roll * 0.5).sin(), (roll * 0.5).cos());
    let (sp, cp) = ((pitch * 0.5).sin(), (pitch * 0.5).cos());
    let (sy, cy) = ((yaw * 0.5).sin(), (yaw * 0.5).cos());

    let x = sr * cp * cy - cr * sp * sy;
    let y = cr * sp * cy + sr * cp * sy;
    let z = cr * cp * sy - sr * sp * cy;
    let w = cr * cp * cy + sr * sp * sy;

    // Rounded to a picoradian-scale precision so that two runs over the same file produce
    // byte-identical output on any platform whose libm differs in the last bit. A descriptor that
    // changes digest because it was generated on a different machine is a descriptor nobody can
    // verify.
    Json::Arr(vec![
        Json::Num(round12(x)),
        Json::Num(round12(y)),
        Json::Num(round12(z)),
        Json::Num(round12(w)),
    ])
}

fn round12(value: f64) -> f64 {
    let scaled = (value * 1e12).round() / 1e12;
    // -0.0 and 0.0 are equal and serialise differently. One spelling only.
    if scaled == 0.0 {
        0.0
    } else {
        scaled
    }
}

fn provenance(options: &ImportOptions) -> Json {
    let mut source = Json::obj();
    source
        .set_opt("file", options.source_file.clone().map(Json::Str))
        .set_opt("sha256", options.source_sha256.clone().map(Json::Str));

    let mut out = Json::obj();
    out.set("tool", Json::Str(URDF_DRAFT_TOOL.to_string()))
        .set("tool_version", Json::Str(options.tool_version.clone()))
        .set("source", source)
        .set(
            "limits",
            Json::Str(
                "Design values read from the URDF. A URDF effort limit is a figure from a CAD \
                 model, not a datasheet safety rating."
                    .to_string(),
            ),
        )
        .set(
            "envelope",
            Json::Str(
                "Not declared. Sourced safety figures must come from the vendor's documentation or \
                 a risk assessment; this tool will not populate them from model values (§63.4)."
                    .to_string(),
            ),
        )
        .set(
            "roles",
            Json::Str(
                if options.accept_suggested_roles {
                    "Suggested from joint-name heuristics and accepted by the operator running the \
                     import. URDF carries no role semantics."
                } else {
                    "Not assigned. URDF carries no role semantics; suggestions were reported for a \
                     human to confirm."
                }
                .to_string(),
            ),
        );
    out
}

fn resolve_slug(model: &UrdfModel, options: &ImportOptions) -> Result<String, ImportError> {
    if let Some(slug) = &options.robot_slug {
        let cleaned = slugify(slug);
        if cleaned.is_empty() {
            return Err(ImportError {
                message: format!("`--slug {slug}` contains no usable characters."),
            });
        }
        return Ok(cleaned);
    }
    let derived = slugify(&model.name);
    if derived.is_empty() {
        return Err(ImportError {
            message:
                "the URDF's `<robot name>` is missing or unusable as a slug, and a descriptor \
                      has to say which robot it describes. Supply one with --slug."
                    .to_string(),
        });
    }
    Ok(derived)
}

/// Lowercase, hyphen-separated, alphanumeric. The Hub's slug rules, applied here so a descriptor
/// generated offline can be published without being renamed.
fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_separator = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('-');
            }
            pending_separator = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_separator = true;
        }
    }
    out
}

/// The base kind, and the one place this tool defaults rather than refusing.
///
/// A URDF states how the model's root link moves *within the model*, and nothing at all about
/// whether the machine drives around a room. Where the model settles it — a `floating` or `planar`
/// joint says the root moves and does not say how — the tool refuses and asks. Where the model is
/// silent, `fixed` is emitted, because it is the conservative reading: a fixed base makes base
/// velocity channels refuse rather than command motion the body may not be able to produce. The
/// report says so, and names the correction.
fn resolve_base(model: &UrdfModel, options: &ImportOptions) -> Result<String, ImportError> {
    if let Some(base) = &options.base {
        if !BASE_KINDS.contains(&base.as_str()) {
            return Err(ImportError {
                message: format!(
                    "`--base {base}` is not a base kind. Valid: {}.",
                    BASE_KINDS.join(", ")
                ),
            });
        }
        return Ok(base.clone());
    }

    if let Some(joint) = model
        .joints
        .iter()
        .find(|joint| matches!(joint.joint_type.as_str(), "floating" | "planar"))
    {
        return Err(ImportError {
            message: format!(
                "joint `{}` is `{}`, so the model says the root link moves through the world and \
                 does not say how it is carried. Declare the base kind: --base {}.",
                joint.name,
                joint.joint_type,
                BASE_KINDS.join("|")
            ),
        });
    }

    Ok("fixed".to_string())
}

fn validate_control_modes(modes: &[String]) -> Result<(), ImportError> {
    for mode in modes {
        if !CONTROL_MODES.contains(&mode.as_str()) {
            return Err(ImportError {
                message: format!(
                    "`{mode}` is not a control mode. Valid: {}.",
                    CONTROL_MODES.join(", ")
                ),
            });
        }
    }
    Ok(())
}

/// A note the report adds when the joints look like legs or wheels and the base was defaulted.
///
/// Exposed as a function rather than folded into [`build`] because it is advice about a value the
/// caller chose, and the CLI is where advice belongs.
pub fn base_advice(model: &UrdfModel, report: &ImportReport, base: &str) -> Option<Note> {
    if base != "fixed" {
        return None;
    }
    let legs = report
        .suggestions
        .iter()
        .filter(|s| s.role.as_deref().is_some_and(|role| role.contains("_leg.")))
        .count();
    let wheels = model
        .actuators()
        .filter(|joint| joint.name.to_ascii_lowercase().contains("wheel"))
        .count();

    if legs == 0 && wheels == 0 {
        return None;
    }
    let evidence = if legs > 0 {
        format!("{legs} joints suggest leg chains")
    } else {
        format!("{wheels} joints are named as wheels")
    };
    Some(Note {
        level: NoteLevel::Action,
        message: format!(
            "`base` was defaulted to `fixed` because the model declares no joint to the world, but \
             {evidence}. If this body moves, re-run with --base legged|wheeled|tracked|aerial|rail|surface|submerged \
             — a fixed base makes every base-velocity channel refuse."
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    /// This module keeps its own copy of the kinds (it resolves `--base` before any contract is
    /// built), so a kind added to the contracts and not here would be refused at the command line.
    #[test]
    fn the_base_kinds_are_the_contracts_own() {
        assert_eq!(
            BASE_KINDS,
            skynet_contracts::embodiment::BASE_KINDS,
            "§97 added `surface` and `submerged`; both lists must say so"
        );
    }

    const ARM: &str = r#"
<robot name="Bench Arm">
  <link name="base_link"><collision/><inertial><mass value="2.5"/></inertial></link>
  <link name="upper"/>
  <link name="tool"/>
  <joint name="shoulder_pitch" type="revolute">
    <parent link="base_link"/><child link="upper"/>
    <origin xyz="0 0 0.1" rpy="0 0 1.5707963267948966"/>
    <axis xyz="0 1 0"/>
    <limit lower="-1.5" upper="1.5" effort="80" velocity="2"/>
  </joint>
  <joint name="tool_mount" type="fixed">
    <parent link="upper"/><child link="tool"/>
  </joint>
</robot>"#;

    fn options() -> ImportOptions {
        ImportOptions {
            tool_version: "test".to_string(),
            ..Default::default()
        }
    }

    /// The stamp this module writes is the stamp readiness reads. A draft is a declaration with no
    /// adapter behind it, so a generated descriptor counts zero adapters — and if either side
    /// restates the string instead of using the constant, this is the test that says so.
    #[test]
    fn a_generated_descriptor_reads_as_a_draft_with_no_adapter() {
        use skynet_contracts::json::descriptor_from_json;
        use skynet_contracts::readiness::subject_from_descriptor;

        let model = model::parse(ARM).unwrap();
        let import = build(&model, &options()).unwrap();
        let descriptor = descriptor_from_json(&import.descriptor).unwrap();
        let subject = subject_from_descriptor(&descriptor, &import.descriptor, Vec::new());
        assert_eq!(subject.adapter_count, 0.0);
    }

    #[test]
    fn the_descriptor_carries_what_the_urdf_declared_and_nothing_else() {
        let model = model::parse(ARM).unwrap();
        let import = build(&model, &options()).unwrap();
        let d = &import.descriptor;

        assert_eq!(d.get("robot_slug").unwrap().as_str(), Some("bench-arm"));
        assert_eq!(d.get("dof").unwrap().as_f64(), Some(1.0));
        assert_eq!(d.get("base").unwrap().as_str(), Some("fixed"));
        assert_eq!(d.get("verified").unwrap(), &Json::Bool(false));

        let joint = &d.get("joints").unwrap().as_arr().unwrap()[0];
        assert_eq!(joint.get("name").unwrap().as_str(), Some("shoulder_pitch"));
        assert_eq!(joint.get("type").unwrap().as_str(), Some("revolute"));
        let limits = joint.get("limits").unwrap();
        assert_eq!(limits.get("effort_max_nm").unwrap().as_f64(), Some(80.0));
        assert_eq!(limits.get("position_min_rad").unwrap().as_f64(), Some(-1.5));
    }

    /// The slug rule the `ros2_control` bridge mirrors in C++ (`read_robot_name`). The two bodies
    /// both sides host are pinned here and in the bridge's selftest, so a driver descriptor and a
    /// drafted one agree on the body; the rest pins the rule's corners. If this rule moves, the
    /// selftest is the other half that moves with it.
    #[test]
    fn the_slug_rule_is_what_the_ros2_control_bridge_mirrors() {
        assert_eq!(slugify("ur5e"), "ur5e");
        assert_eq!(slugify("two_joint_mock"), "two-joint-mock");
        assert_eq!(slugify("mixed_interfaces"), "mixed-interfaces");
        assert_eq!(slugify("Bench Arm"), "bench-arm");
        assert_eq!(slugify("UR5e cell #3"), "ur5e-cell-3");
        assert_eq!(slugify("___"), "");
    }

    /// The rule with no flag behind it. Everything else in this module is a default; this is a
    /// property.
    #[test]
    fn the_envelope_is_null_whatever_the_caller_asks_for() {
        let model = model::parse(ARM).unwrap();
        let mut opts = options();
        opts.accept_suggested_roles = true;
        opts.assume_side = Some(Side::Left);
        opts.control_modes = vec!["position".to_string(), "torque".to_string()];
        opts.base = Some("wheeled".to_string());

        let import = build(&model, &opts).unwrap();
        assert_eq!(import.descriptor.get("envelope").unwrap(), &Json::Null);
        // The operator's mode assertion reaches both levels, because a plan needs both and a body
        // profile with empty per-joint lists blocks every channel it would otherwise bind.
        let joint = &import.descriptor.get("joints").unwrap().as_arr().unwrap()[0];
        assert_eq!(joint.str_arr("control_modes"), ["position", "torque"]);
        assert!(import
            .report
            .notes
            .iter()
            .any(|note| note.level == NoteLevel::Action
                && note.message.contains("no flag changes that")));
    }

    /// Roles are null by default and the suggestions are still reported, which is the whole shape
    /// of §63.4: suggested, never assigned, and a human confirms.
    #[test]
    fn roles_are_null_until_a_human_accepts_them() {
        let model = model::parse(ARM).unwrap();

        let mut opts = options();
        opts.assume_side = Some(Side::Left);
        let import = build(&model, &opts).unwrap();
        let joint = &import.descriptor.get("joints").unwrap().as_arr().unwrap()[0];
        assert_eq!(joint.get("role").unwrap(), &Json::Null);
        assert_eq!(import.report.suggested_count(), 1);
        assert_eq!(
            import.report.suggestions[0].role.as_deref(),
            Some("left_arm.shoulder.pitch")
        );

        opts.accept_suggested_roles = true;
        let accepted = build(&model, &opts).unwrap();
        let joint = &accepted.descriptor.get("joints").unwrap().as_arr().unwrap()[0];
        assert_eq!(
            joint.get("role").unwrap().as_str(),
            Some("left_arm.shoulder.pitch")
        );
        assert!(accepted.report.roles_accepted);
    }

    /// A joint the URDF gave no rating gets no rating. Absent is not zero and is not unlimited.
    #[test]
    fn an_undeclared_rating_is_absent_from_the_output() {
        let model = model::parse(
            r#"<robot name="r"><link name="a"/><link name="b"/>
               <joint name="j" type="continuous"><parent link="a"/><child link="b"/></joint></robot>"#,
        )
        .unwrap();
        let import = build(&model, &options()).unwrap();
        let limits = import.descriptor.get("joints").unwrap().as_arr().unwrap()[0]
            .get("limits")
            .unwrap();
        assert_eq!(limits, &Json::obj());
        assert!(limits.get("effort_max_nm").is_none());
    }

    #[test]
    fn the_frame_graph_names_the_joint_that_drives_each_moving_frame() {
        let model = model::parse(ARM).unwrap();
        let import = build(&model, &options()).unwrap();
        let frames = import.descriptor.get("frames").unwrap();
        assert_eq!(frames.get("root").unwrap().as_str(), Some("base_link"));
        assert_eq!(frames.get("convention").unwrap().as_str(), Some("rep103"));

        let nodes = frames.get("nodes").unwrap().as_arr().unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].get("kind").unwrap().as_str(), Some("root"));
        assert_eq!(nodes[0].get("collision").unwrap(), &Json::Bool(true));
        assert_eq!(nodes[0].get("mass_kg").unwrap().as_f64(), Some(2.5));

        assert_eq!(nodes[1].get("kind").unwrap().as_str(), Some("dynamic"));
        assert_eq!(
            nodes[1].get("joint").unwrap().as_str(),
            Some("shoulder_pitch")
        );
        assert_eq!(nodes[1].get("parent").unwrap().as_str(), Some("base_link"));

        // A fixed joint is a frame and not a degree of freedom.
        assert_eq!(nodes[2].get("kind").unwrap().as_str(), Some("static"));
        assert!(nodes[2].get("joint").is_none());
    }

    /// A quarter turn about z. Getting the composition order wrong produces a frame graph that is
    /// consistently, subtly wrong, which is far worse than one that is obviously broken.
    #[test]
    fn rpy_becomes_an_xyzw_quaternion_in_the_urdf_composition_order() {
        let model = model::parse(ARM).unwrap();
        let import = build(&model, &options()).unwrap();
        let nodes = import
            .descriptor
            .get("frames")
            .unwrap()
            .get("nodes")
            .unwrap();
        let rotation = nodes.as_arr().unwrap()[1]
            .get("rotation")
            .unwrap()
            .as_arr()
            .unwrap();
        let values: Vec<f64> = rotation.iter().filter_map(Json::as_f64).collect();
        let half = core::f64::consts::FRAC_1_SQRT_2;
        assert!((values[0] - 0.0).abs() < 1e-9, "{values:?}");
        assert!((values[1] - 0.0).abs() < 1e-9, "{values:?}");
        assert!((values[2] - half).abs() < 1e-9, "{values:?}");
        assert!((values[3] - half).abs() < 1e-9, "{values:?}");
    }

    /// F168. What a URDF writes to mean "unlimited" or "unknown" is left out, and named, rather
    /// than copied into a descriptor as a rating — the published EVE r3 declares 10³⁰ N·m. Held to
    /// a control: the real figures on the same file survive.
    #[test]
    fn a_sentinel_is_not_a_rating() {
        let urdf = r#"<robot name="sentinels">
          <link name="base"/><link name="wheel"/><link name="arm"/>
          <joint name="wheel_joint" type="revolute"><parent link="base"/><child link="wheel"/>
            <axis xyz="0 1 0"/><limit lower="-1e16" upper="1e16" effort="1e30" velocity="-1"/></joint>
          <joint name="arm_joint" type="revolute"><parent link="base"/><child link="arm"/>
            <axis xyz="0 1 0"/><limit lower="-1.5" upper="0.5" effort="40" velocity="3"/></joint>
        </robot>"#;
        let model = model::parse(urdf).unwrap();
        let import = build(&model, &options()).unwrap();
        let joints = import.descriptor.get("joints").unwrap().as_arr().unwrap();
        let limits = |index: usize| joints[index].get("limits").unwrap().clone();
        for key in [
            "position_min_rad",
            "position_max_rad",
            "effort_max_nm",
            "velocity_max_rad_s",
        ] {
            assert!(
                limits(0).get(key).is_none(),
                "{key} was copied from a sentinel"
            );
        }
        assert_eq!(
            limits(1).get("position_min_rad").unwrap().as_f64(),
            Some(-1.5)
        );
        assert_eq!(limits(1).get("effort_max_nm").unwrap().as_f64(), Some(40.0));
        assert!(
            (import.report.notes.iter()).any(|note| note
                .message
                .contains("`wheel_joint` (effort, velocity, travel)")),
            "the report names the joint and the figures it left out"
        );
    }

    /// Generating the same file twice must produce the same bytes, or the digest a vendor signs
    /// depends on which machine ran the tool.
    #[test]
    fn the_same_urdf_produces_the_same_bytes() {
        let model = model::parse(ARM).unwrap();
        let first = build(&model, &options())
            .unwrap()
            .descriptor
            .to_canonical()
            .unwrap();
        let second = build(&model, &options())
            .unwrap()
            .descriptor
            .to_canonical()
            .unwrap();
        assert_eq!(first, second);
        assert!(first.contains("\"envelope\":null"));
    }

    #[test]
    fn a_floating_root_refuses_rather_than_guessing_the_base() {
        let model = model::parse(
            r#"<robot name="quad"><link name="world"/><link name="body"/>
               <joint name="float" type="floating"><parent link="world"/><child link="body"/></joint>
               </robot>"#,
        )
        .unwrap();
        let err = build(&model, &options()).unwrap_err();
        assert!(
            err.message.contains("does not say how it is carried"),
            "{}",
            err.message
        );
        assert!(err.message.contains("--base"), "{}", err.message);

        let mut opts = options();
        opts.base = Some("legged".to_string());
        assert_eq!(
            build(&model, &opts)
                .unwrap()
                .descriptor
                .get("base")
                .unwrap()
                .as_str(),
            Some("legged")
        );
    }

    #[test]
    fn bad_operator_input_is_refused_with_the_valid_set() {
        let model = model::parse(ARM).unwrap();

        let mut opts = options();
        opts.base = Some("hovering".to_string());
        assert!(build(&model, &opts)
            .unwrap_err()
            .message
            .contains("wheeled"));

        let mut opts = options();
        opts.control_modes = vec!["impedance".to_string()];
        let err = build(&model, &opts).unwrap_err();
        assert!(
            err.message.contains("`impedance` is not a control mode"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_robot_with_no_usable_name_asks_for_a_slug() {
        let model = model::parse(r#"<robot name="!!!"><link name="a"/></robot>"#).unwrap();
        let err = build(&model, &options()).unwrap_err();
        assert!(err.message.contains("--slug"), "{}", err.message);
    }

    #[test]
    fn a_leg_bearing_model_with_a_defaulted_base_gets_advice() {
        let model = model::parse(
            r#"<robot name="quad"><link name="body"/><link name="l"/>
               <joint name="left_hip_pitch" type="revolute"><parent link="body"/><child link="l"/>
                 <limit lower="-1" upper="1" effort="1" velocity="1"/></joint></robot>"#,
        )
        .unwrap();
        let import = build(&model, &options()).unwrap();
        let advice = base_advice(&model, &import.report, "fixed").unwrap();
        assert_eq!(advice.level, NoteLevel::Action);
        assert!(
            advice.message.contains("suggest leg chains"),
            "{}",
            advice.message
        );
        assert!(base_advice(&model, &import.report, "legged").is_none());
    }
}
