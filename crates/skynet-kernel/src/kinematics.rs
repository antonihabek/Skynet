//! Forward kinematics, and the geometry the two pose guards are made of.
//!
//! > **The kernel computes its own forward kinematics and subscribes to nothing.** (§44.2)
//!
//! That sentence is the whole of this module's design. A transform tree distributed over a message
//! bus — ROS `tf2` being the canonical example — has unbounded staleness and no delivery guarantee,
//! and a `workspace` guard evaluating a 200 ms old transform is not a degraded guard, it is a
//! confidently wrong one. The kernel already holds the joint vector it just read; forward
//! kinematics over a fixed chain is a few hundred floating-point operations and is deterministic.
//! So it computes them, in cycle, from the state it just read, and its staleness is zero by
//! construction.
//!
//! # The shape of the data, and why it is not the descriptor's shape
//!
//! §44.3's frame graph is a list of nodes naming their parents by string. That is the right shape
//! for a signed document a human reads and two generators write. It is the wrong shape for a
//! control cycle: resolving a parent by name is a string comparison against every other frame, on
//! every frame, on every cycle.
//!
//! So the graph is **flattened once**, when the lease is granted, into [`Frame`]s carrying a
//! **parent index** that is strictly smaller than their own. Three properties fall out, and each
//! removes a failure mode rather than only some work:
//!
//! * Forward kinematics is a single forward pass over a slice. No recursion, so no stack depth to
//!   bound on a safety MCU.
//! * A cycle in the graph is **unrepresentable**, rather than being a hang in the middle of a
//!   control cycle. [`FrameSet::new`] refuses an order that does not have this property, so the
//!   check is paid once and never again.
//! * A parent's pose is always already computed when its child is reached, so there is no
//!   "resolved yet?" bookkeeping in the inner loop beyond one boolean read.
//!
//! # What is deliberately absent
//!
//! **No inverse kinematics.** §44.2 gives the kernel forward kinematics from joint state and
//! nothing else; there is no solver here, and the `workspace` guard does not need one — it checks
//! where a commanded configuration *puts* a frame, which is forward.
//!
//! **No allocation and no clock**, as everywhere else in this crate. [`Kinematics`] is a
//! caller-owned scratch buffer, filled in place.
//!
//! # What it costs, in numbers rather than in adjectives
//!
//! Forward kinematics is linear: one quaternion product, one rotation of a vector and one joint
//! motion per frame, so a 40-frame humanoid is a few thousand operations. Self-collision is
//! **quadratic in the number of collision bodies** — 40 of them is 780 pairs, and each pair is a
//! segment-to-segment distance of roughly sixty operations, so about fifty thousand per cycle.
//!
//! That is comfortable on an application processor and it is *not* comfortable on a safety MCU
//! without double-precision hardware, where every one of those operations is a software routine
//! costing tens of cycles. [`FrameSet::collision_pairs`] reports the number so a surface can show
//! it rather than a reader assuming it is small, and §7.4's certified tier will have to answer the
//! question before it mirrors this guard onto a second processor.

use crate::fixed::FixedVec;
use crate::real::{abs, cos, sin, sqrt};

/// Largest number of frames one body may declare.
///
/// A 30-joint humanoid has roughly 40 links once tool frames and sensor frames are counted, so 128
/// leaves the same headroom [`crate::MAX_CHANNELS`] does. Exceeding it is refused at flatten time,
/// never truncated: half a frame graph is a robot whose arm is missing from its own collision
/// model.
pub const MAX_FRAMES: usize = 128;

/// Largest number of pose-guard trips reported in one cycle.
///
/// The response does not depend on how many there were — one trip and twenty both `hold` — so this
/// bounds the *report* and not the decision. [`PoseOutcome::truncated`] says when it was reached.
pub const MAX_POSE_TRIPS: usize = 8;

/// Largest number of frames named as unresolved in one cycle.
pub const MAX_UNRESOLVED: usize = 8;

// ── Rigid transforms ─────────────────────────────────────────────────────────

/// A rigid transform: a translation in metres and a rotation as a quaternion ordered `xyzw`.
///
/// §44.4's conventions, in a struct: right-handed, metres, `xyzw`, and rotations are **active** —
/// they are applied to points, not to frames. Getting that last one backwards inverts every
/// rotation in the body and produces a robot that is confidently wrong about where its own hand is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Metres, in the parent frame.
    pub translation: [f64; 3],
    /// Ordered `xyzw` (§44.4).
    pub rotation: [f64; 4],
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    /// No translation and no rotation.
    pub const IDENTITY: Transform = Transform {
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };

    /// `self` followed by `local`: the transform of a child expressed in this frame's parent.
    ///
    /// Order matters and is stated rather than implied: the child's translation is rotated **into**
    /// this frame before being added, which is what makes a chain of these a kinematic chain rather
    /// than a sum of offsets.
    pub fn compose(&self, local: &Transform) -> Transform {
        let rotated = rotate(&self.rotation, &local.translation);
        Transform {
            translation: [
                self.translation[0] + rotated[0],
                self.translation[1] + rotated[1],
                self.translation[2] + rotated[2],
            ],
            rotation: quaternion_product(&self.rotation, &local.rotation),
        }
    }

    /// Whether every component is finite.
    ///
    /// A NaN reaches here from exactly one place — [`crate::real::sin`] refusing an angle outside
    /// the range its reduction is exact over — and it must not propagate into a guard verdict. A
    /// frame whose pose is not finite is an unresolved frame, and is named.
    pub fn is_finite(&self) -> bool {
        self.translation.iter().all(|v| v.is_finite())
            && self.rotation.iter().all(|v| v.is_finite())
    }
}

/// Hamilton product, `xyzw` in and `xyzw` out.
fn quaternion_product(a: &[f64; 4], b: &[f64; 4]) -> [f64; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Rotate a point by a quaternion. Active rotation, per §44.4.
fn rotate(q: &[f64; 4], v: &[f64; 3]) -> [f64; 3] {
    // t = 2 (q_vec × v); v' = v + q_w t + q_vec × t. Fifteen multiplications and no trigonometry,
    // and it does not assume the quaternion is normalised beyond what the caller guarantees.
    let (qx, qy, qz, qw) = (q[0], q[1], q[2], q[3]);
    let tx = 2.0 * (qy * v[2] - qz * v[1]);
    let ty = 2.0 * (qz * v[0] - qx * v[2]);
    let tz = 2.0 * (qx * v[1] - qy * v[0]);
    [
        v[0] + qw * tx + (qy * tz - qz * ty),
        v[1] + qw * ty + (qz * tx - qx * tz),
        v[2] + qw * tz + (qx * ty - qy * tx),
    ]
}

// ── Joints ───────────────────────────────────────────────────────────────────

/// The joint kinds §44.3 admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointKind {
    /// One rotational degree of freedom about the declared axis, with travel limits.
    Revolute,
    /// The same, unbounded.
    Continuous,
    /// One translational degree of freedom along the declared axis.
    Prismatic,
    /// Three rotational degrees of freedom.
    Ball,
    /// Six.
    Floating,
}

impl JointKind {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            JointKind::Revolute => "revolute",
            JointKind::Continuous => "continuous",
            JointKind::Prismatic => "prismatic",
            JointKind::Ball => "ball",
            JointKind::Floating => "floating",
        }
    }

    /// Parse the wire spelling. An unknown kind is `None` rather than a default: guessing that an
    /// unrecognised joint is revolute is how a prismatic axis becomes a rotation.
    pub fn parse(text: &str) -> Option<JointKind> {
        match text {
            "revolute" => Some(JointKind::Revolute),
            "continuous" => Some(JointKind::Continuous),
            "prismatic" => Some(JointKind::Prismatic),
            "ball" => Some(JointKind::Ball),
            "floating" => Some(JointKind::Floating),
            _ => None,
        }
    }

    /// Whether one scalar joint value parameterises this kind.
    ///
    /// Ball and floating joints are the two that a scalar cannot describe, and §44.3's frame graph
    /// carries one value per joint. A frame driven by one is therefore **unresolvable** — reported,
    /// never approximated by ignoring the missing degrees of freedom, because an arm whose shoulder
    /// is assumed not to have rolled is an arm in a place it is not.
    pub fn is_scalar(self) -> bool {
        matches!(
            self,
            JointKind::Revolute | JointKind::Continuous | JointKind::Prismatic
        )
    }
}

/// A joint that follows another rather than being commanded (§44.3's `mimics`).
///
/// The multiplier and offset are URDF's, and they are here rather than assumed to be 1 and 0
/// because the commonest linkage in robotics is a parallel gripper whose second finger runs at −1.
/// A field that could only express a ratio of one would put that finger on the wrong side of the
/// object, in a guard whose whole job is to know where things are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mimic<'a> {
    /// The joint this one follows.
    pub joint: &'a str,
    /// `value = multiplier * driver + offset`.
    pub multiplier: f64,
    /// Radians or metres, matching the joint kind.
    pub offset: f64,
}

/// The moving part of a dynamic frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Joint<'a> {
    /// The joint's name, as the command and the state key it.
    pub name: &'a str,
    /// What kind of motion it is.
    pub kind: JointKind,
    /// The axis, in this frame, before the joint is applied (§44.3).
    pub axis: [f64; 3],
    /// Set when this joint is a linkage rather than a degree of freedom.
    pub mimic: Option<Mimic<'a>>,
}

/// Where a joint's value came from this cycle.
///
/// The distinction is the difference between the guard §7.1 specifies — "commanded configuration,
/// one step lookahead" — and a weaker check of where the body is right now. Both are worth running;
/// only one of them is what the specification promises, so the verdict says which was done.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointBinding<'a> {
    /// The joint, as the frame graph names it.
    pub joint: &'a str,
    /// The command channel that drives it, when the transport plan resolved one to a **position**.
    ///
    /// `None` on a body commanded in velocity or torque, where next cycle's configuration is not a
    /// function of this cycle's command and the kernel will not pretend otherwise.
    pub channel: Option<&'a str>,
    /// Where the joint is now, from the state the RHAL just read.
    pub measured: f64,
}

// ── Collision geometry ───────────────────────────────────────────────────────

/// A sphere-swept segment: every point within `radius_m` of the segment from `a` to `b`.
///
/// One primitive rather than a shape library, and it is the right one. A capsule is a sphere when
/// `a == b`, it bounds a cylinder exactly, it bounds a link's real geometry conservatively, and the
/// distance between two of them is a closed form with no branching beyond four clamps. A collision
/// guard built on meshes would need a broadphase, a solver and an allocator, none of which belong
/// inside a control cycle.
///
/// Conservative is the correct direction of error: a capsule contains the geometry it stands for,
/// so the guard trips early rather than late.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    /// One end of the segment, in the frame's own coordinates.
    pub a: [f64; 3],
    /// The other end. Equal to `a` for a sphere.
    pub b: [f64; 3],
    /// Metres.
    pub radius_m: f64,
}

// ── The frame set ────────────────────────────────────────────────────────────

/// One frame, flattened for the control cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame<'a> {
    /// The frame's name, for naming it in a verdict.
    pub name: &'a str,
    /// Index of the parent, strictly less than this frame's own. `None` on the root only.
    pub parent: Option<u16>,
    /// The fixed part of the transform from the parent.
    pub local: Transform,
    /// The moving part, on a dynamic frame.
    pub joint: Option<Joint<'a>>,
    /// Collision geometry, when the body declares a shape this kernel can check.
    pub collision: Option<Capsule>,
    /// Frames this one is permitted to touch, by index.
    ///
    /// Every real body has some. A humanoid's pelvis and torso overlap by design, and a guard with
    /// no way to say so trips on the robot standing still — which is the fastest route to an
    /// operator switching the guard off, the failure §7.1 spends a paragraph avoiding. The list is
    /// indices rather than names because it is read inside the quadratic loop.
    pub ignore: &'a [u16],
}

/// Why a flattened frame set was refused.
///
/// Refused rather than repaired: every one of these is a body description that would otherwise
/// produce a pose the guard believes and the robot does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSetFault {
    /// No frames at all.
    Empty,
    /// More frames than [`MAX_FRAMES`].
    TooLarge,
    /// The first frame is not the root, or a later frame claims to be one.
    RootNotFirst,
    /// A parent index is not strictly smaller than its child's, so a single forward pass would read
    /// a pose that had not been computed.
    NotTopological,
    /// A parent index is past the end of the slice.
    ParentOutOfRange,
    /// An ignore-list entry is past the end of the slice.
    IgnoreOutOfRange,
}

impl FrameSetFault {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            FrameSetFault::Empty => "empty",
            FrameSetFault::TooLarge => "too_large",
            FrameSetFault::RootNotFirst => "root_not_first",
            FrameSetFault::NotTopological => "not_topological",
            FrameSetFault::ParentOutOfRange => "parent_out_of_range",
            FrameSetFault::IgnoreOutOfRange => "ignore_out_of_range",
        }
    }
}

/// A body's frames, ordered so that forward kinematics is one forward pass.
#[derive(Debug, Clone, Copy)]
pub struct FrameSet<'a> {
    frames: &'a [Frame<'a>],
}

impl<'a> FrameSet<'a> {
    /// Check the ordering invariant once, so that no cycle ever checks it again.
    ///
    /// The invariant is the whole reason this type exists: **frame `i`'s parent index is `< i`**.
    /// A graph with a cycle cannot satisfy it, so a cycle is refused here rather than discovered as
    /// a control loop that missed its deadline.
    pub fn new(frames: &'a [Frame<'a>]) -> Result<FrameSet<'a>, FrameSetFault> {
        if frames.is_empty() {
            return Err(FrameSetFault::Empty);
        }
        if frames.len() > MAX_FRAMES {
            return Err(FrameSetFault::TooLarge);
        }
        if frames[0].parent.is_some() {
            return Err(FrameSetFault::RootNotFirst);
        }
        for (index, frame) in frames.iter().enumerate() {
            match frame.parent {
                None if index > 0 => return Err(FrameSetFault::RootNotFirst),
                None => {}
                Some(parent) => {
                    let parent = parent as usize;
                    if parent >= frames.len() {
                        return Err(FrameSetFault::ParentOutOfRange);
                    }
                    if parent >= index {
                        return Err(FrameSetFault::NotTopological);
                    }
                }
            }
            for ignored in frame.ignore {
                if *ignored as usize >= frames.len() {
                    return Err(FrameSetFault::IgnoreOutOfRange);
                }
            }
        }
        Ok(FrameSet { frames })
    }

    /// The frames, in topological order.
    pub fn frames(&self) -> &'a [Frame<'a>] {
        self.frames
    }

    /// How many frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether there are none. Never true for a constructed set; present because clippy asks.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The index of a named frame.
    pub fn index_of(&self, name: &str) -> Option<u16> {
        self.frames
            .iter()
            .position(|frame| frame.name == name)
            .map(|index| index as u16)
    }

    /// Whether this frame's pose is computable at all, whatever the joint values are.
    ///
    /// False when the walk to the root passes through a joint one scalar cannot describe. That is a
    /// **static** property of the body, and knowing it statically is what turns "the guard holds the
    /// robot on every cycle" into "the guard reported itself unavailable when the lease was
    /// granted, naming the frame" — which is §44.5's requirement and the difference between a
    /// robot that will not move and an operator who knows why.
    pub fn statically_resolvable(&self, index: usize) -> bool {
        let mut here = index;
        loop {
            let frame = match self.frames.get(here) {
                Some(frame) => frame,
                None => return false,
            };
            if let Some(joint) = &frame.joint {
                if !joint.kind.is_scalar() {
                    return false;
                }
            }
            match frame.parent {
                None => return true,
                // The ordering invariant makes this walk terminate: every step strictly decreases.
                Some(parent) => here = parent as usize,
            }
        }
    }

    /// How many frames carry collision geometry this kernel can check.
    ///
    /// Only the ones whose pose can be computed. A capsule on a frame hanging off a ball joint is
    /// geometry nobody can place, and counting it would report a guard as available that has
    /// nothing to test.
    pub fn collision_bodies(&self) -> usize {
        self.frames
            .iter()
            .enumerate()
            .filter(|(index, frame)| {
                frame.collision.is_some() && self.statically_resolvable(*index)
            })
            .count()
    }

    /// How many pairs the self-collision guard tests each cycle.
    ///
    /// Reported rather than left to be discovered, because it is the one cost in this kernel that
    /// grows quadratically and the one a certified-tier port has to fit onto a second processor.
    pub fn collision_pairs(&self) -> usize {
        let n = self.collision_bodies();
        n.saturating_mul(n.saturating_sub(1)) / 2
    }
}

// ── Solving ──────────────────────────────────────────────────────────────────

/// The computed pose of every frame, and what could not be computed.
///
/// Owned by the caller and filled in place: at the host tier this sits in the control thread's
/// preallocated frame, and at the certified tier in the MCU's static storage. Roughly seven
/// kilobytes, which is the price of not allocating.
#[derive(Debug, Clone)]
pub struct Kinematics {
    poses: [Transform; MAX_FRAMES],
    resolved: [bool; MAX_FRAMES],
    len: usize,
    unresolved: FixedVec<u16, MAX_UNRESOLVED>,
    lookahead: bool,
}

impl Default for Kinematics {
    fn default() -> Self {
        Self::new()
    }
}

impl Kinematics {
    /// An empty scratch buffer.
    pub fn new() -> Kinematics {
        Kinematics {
            poses: [Transform::IDENTITY; MAX_FRAMES],
            resolved: [false; MAX_FRAMES],
            len: 0,
            unresolved: FixedVec::new(),
            lookahead: true,
        }
    }

    /// Compute every frame's pose in the root frame.
    ///
    /// One forward pass. Each frame composes its parent's pose with its own fixed transform and,
    /// where it is dynamic, with the motion its joint value implies. A frame is **unresolved** when
    /// its parent is, when its joint has no value, when its joint kind needs more than a scalar, or
    /// when the arithmetic produced something that is not finite — and unresolved propagates down
    /// the chain, because a pose relative to an unknown pose is unknown.
    ///
    /// `command` is a lookup rather than a slice. The kernel's command frame is a fixed-capacity
    /// vector with no contiguous backing, and copying it into one every cycle to hand it here would
    /// be three kilobytes of memcpy on the hot path to avoid a closure.
    ///
    /// The configuration is built per joint from [`JointBinding`]: the post-clamp command where the
    /// transport plan bound a position channel, and the measured state otherwise. [`Self::lookahead`]
    /// records whether every joint came from the command, because "the configuration this command
    /// produces" and "the configuration the robot is in" are different claims and §7.1 promises the
    /// first.
    pub fn solve(
        &mut self,
        set: &FrameSet<'_>,
        bindings: &[JointBinding<'_>],
        command: impl Fn(&str) -> Option<f64>,
    ) {
        self.len = set.len();
        self.unresolved = FixedVec::new();
        self.lookahead = true;

        for (index, frame) in set.frames().iter().enumerate() {
            let parent_pose = match frame.parent {
                None => Transform::IDENTITY,
                Some(parent) => {
                    let parent = parent as usize;
                    if !self.resolved[parent] {
                        self.mark_unresolved(index);
                        continue;
                    }
                    self.poses[parent]
                }
            };

            let mut local = frame.local;
            if let Some(joint) = &frame.joint {
                match self.joint_value(joint, bindings, &command) {
                    None => {
                        self.mark_unresolved(index);
                        continue;
                    }
                    Some(value) => match motion(joint, value) {
                        None => {
                            self.mark_unresolved(index);
                            continue;
                        }
                        Some(motion) => local = local.compose(&motion),
                    },
                }
            }

            let pose = parent_pose.compose(&local);
            if !pose.is_finite() {
                self.mark_unresolved(index);
                continue;
            }
            self.poses[index] = pose;
            self.resolved[index] = true;
        }
    }

    fn mark_unresolved(&mut self, index: usize) {
        self.resolved[index] = false;
        self.unresolved.push(index as u16);
    }

    /// The value driving one joint this cycle, following a mimic to its driver.
    ///
    /// A mimic is followed exactly one step. URDF permits no more, and a chain of them would need a
    /// traversal with its own cycle check inside the control loop — which is the thing this module
    /// exists to keep out of the hot path.
    fn joint_value(
        &mut self,
        joint: &Joint<'_>,
        bindings: &[JointBinding<'_>],
        command: &impl Fn(&str) -> Option<f64>,
    ) -> Option<f64> {
        match joint.mimic {
            None => self.value_of(joint.name, bindings, command),
            Some(mimic) => {
                let driver = self.value_of(mimic.joint, bindings, command)?;
                Some(mimic.multiplier * driver + mimic.offset)
            }
        }
    }

    fn value_of(
        &mut self,
        joint: &str,
        bindings: &[JointBinding<'_>],
        command: &impl Fn(&str) -> Option<f64>,
    ) -> Option<f64> {
        let binding = bindings.iter().find(|entry| entry.joint == joint)?;
        match binding.channel {
            Some(channel) => match command(channel) {
                Some(value) => Some(value),
                // The plan bound a channel and this cycle's command did not carry it. The joint is
                // where the state says, and this is no longer a lookahead.
                None => {
                    self.lookahead = false;
                    Some(binding.measured)
                }
            },
            None => {
                self.lookahead = false;
                Some(binding.measured)
            }
        }
    }

    /// The pose of frame `index`, when it resolved.
    pub fn pose(&self, index: usize) -> Option<&Transform> {
        if index >= self.len || !self.resolved[index] {
            return None;
        }
        Some(&self.poses[index])
    }

    /// Indices of frames with no pose this cycle.
    pub fn unresolved(&self) -> &FixedVec<u16, MAX_UNRESOLVED> {
        &self.unresolved
    }

    /// Whether every joint value came from this cycle's command.
    ///
    /// False means the check that ran was of the configuration the body is in, not the one the
    /// command would produce — a real check, and a weaker one than §7.1 names.
    pub fn lookahead(&self) -> bool {
        self.lookahead
    }
}

/// The transform a joint value produces, in the joint's own frame.
///
/// `None` where a scalar cannot describe the motion, or where the axis is degenerate. A zero-length
/// axis is a body description defect and not a fixed joint: reading it as one would silently weld a
/// limb in the guard's model of the robot while it moved in the world.
fn motion(joint: &Joint<'_>, value: f64) -> Option<Transform> {
    if !value.is_finite() {
        return None;
    }
    if !joint.kind.is_scalar() {
        return None;
    }
    let axis = unit(&joint.axis)?;
    match joint.kind {
        JointKind::Prismatic => Some(Transform {
            translation: [axis[0] * value, axis[1] * value, axis[2] * value],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }),
        // Revolute and continuous differ only in whether travel is bounded, which is the envelope
        // guard's business and not this one's.
        _ => {
            let half = 0.5 * value;
            let s = sin(half);
            let c = cos(half);
            if !s.is_finite() || !c.is_finite() {
                return None;
            }
            Some(Transform {
                translation: [0.0, 0.0, 0.0],
                rotation: [axis[0] * s, axis[1] * s, axis[2] * s, c],
            })
        }
    }
}

/// Normalise, or `None` when there is nothing to normalise.
fn unit(v: &[f64; 3]) -> Option<[f64; 3]> {
    let squared = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if !squared.is_finite() || squared <= 0.0 {
        return None;
    }
    let length = sqrt(squared);
    if !length.is_finite() || length <= 0.0 {
        return None;
    }
    Some([v[0] / length, v[1] / length, v[2] / length])
}

// ── Volumes ──────────────────────────────────────────────────────────────────

/// An operator-defined region, in the root frame.
///
/// **In the root frame, and that is a real limitation.** §44.2 gives the kernel a frame set rooted
/// at `base_link` and no world pose — localisation lives in the perception tree, which the kernel
/// deliberately does not subscribe to. So a volume is a region relative to the robot's own base.
/// For a fixed-base arm that is identical to a region of the cell, which is what an operator
/// means. For a mobile base it is not, and a "stay inside this cell" volume expressed this way
/// travels with the robot. §44.5 is amended to say so rather than leaving an operator to discover
/// it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// An axis-aligned box, `min` and `max` in metres.
    Box {
        /// Lower corner.
        min: [f64; 3],
        /// Upper corner.
        max: [f64; 3],
    },
    /// A sphere.
    Sphere {
        /// Centre, in metres.
        centre: [f64; 3],
        /// Metres.
        radius_m: f64,
    },
}

impl Shape {
    /// Signed distance from a point to this volume's boundary: **positive inside**.
    ///
    /// One sign convention across both pose guards — negative is a violation — so a reader of a
    /// verdict does not have to remember which guard measures which way.
    pub fn margin(&self, point: &[f64; 3]) -> f64 {
        match self {
            Shape::Sphere { centre, radius_m } => {
                let dx = point[0] - centre[0];
                let dy = point[1] - centre[1];
                let dz = point[2] - centre[2];
                radius_m - sqrt(dx * dx + dy * dy + dz * dz)
            }
            Shape::Box { min, max } => {
                let mut outside = [0.0f64; 3];
                let mut any_outside = false;
                let mut inside_margin = f64::INFINITY;
                for axis in 0..3 {
                    let below = min[axis] - point[axis];
                    let above = point[axis] - max[axis];
                    if below > 0.0 {
                        outside[axis] = below;
                        any_outside = true;
                    } else if above > 0.0 {
                        outside[axis] = above;
                        any_outside = true;
                    }
                    let to_low = point[axis] - min[axis];
                    let to_high = max[axis] - point[axis];
                    let nearest = if to_low < to_high { to_low } else { to_high };
                    if nearest < inside_margin {
                        inside_margin = nearest;
                    }
                }
                if any_outside {
                    -sqrt(
                        outside[0] * outside[0] + outside[1] * outside[1] + outside[2] * outside[2],
                    )
                } else {
                    inside_margin
                }
            }
        }
    }
}

/// One volume, and the frame whose origin has to be in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume<'a> {
    /// The frame this volume constrains.
    pub frame: &'a str,
    /// Where it may be.
    pub shape: Shape,
}

/// A layer of the operator's workspace configuration (§38.2).
///
/// Layers compose by **conjunction**: a frame must be inside at least one volume of *every* layer
/// that names it. That is §38.2's "each layer may only narrow" expressed as a predicate rather than
/// as geometry — intersecting a box with a sphere is a shape neither of them is, and a composition
/// rule that produced a third shape would be one no operator could check by reading their own
/// configuration. Conjunction gives the same guarantee with none of that: a later layer can forbid
/// more and can never permit what an earlier one forbade.
///
/// A layer constrains only the frames it names, so a site layer about one arm does not silently
/// confine the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceLayer<'a> {
    /// Which layer this is — `fleet`, `site`, `cell`, a session override — for naming it.
    pub name: &'a str,
    /// The volumes it declares.
    pub volumes: &'a [Volume<'a>],
}

// ── Distance between two capsules ────────────────────────────────────────────

/// Closest distance between two segments, and thereby between two capsules.
///
/// The standard clamped-parameter solution, with the degenerate cases handled rather than assumed
/// away: a sphere is a segment of zero length and every real body has some.
///
/// The **decision** the self-collision guard makes never calls this — it compares squared distances
/// against a squared threshold, so no square root can move a verdict. This computes the number the
/// verdict *reports*, where the root has to be taken and the two editions take the same one.
pub fn segment_distance_squared(p1: &[f64; 3], q1: &[f64; 3], p2: &[f64; 3], q2: &[f64; 3]) -> f64 {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = dot(&d1, &d1);
    let e = dot(&d2, &d2);
    let f = dot(&d2, &r);

    let epsilon = 1e-18;
    let (mut s, mut t);
    if a <= epsilon && e <= epsilon {
        // Two spheres.
        return dot(&r, &r);
    }
    if a <= epsilon {
        s = 0.0;
        t = clamp01(f / e);
    } else {
        let c = dot(&d1, &r);
        if e <= epsilon {
            t = 0.0;
            s = clamp01(-c / a);
        } else {
            let b = dot(&d1, &d2);
            let denominator = a * e - b * b;
            s = if denominator > epsilon {
                clamp01((b * f - c * e) / denominator)
            } else {
                // Parallel: any `s` gives the same distance, so take the start and let `t` follow.
                0.0
            };
            t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = clamp01(-c / a);
            } else if t > 1.0 {
                t = 1.0;
                s = clamp01((b - c) / a);
            }
        }
    }

    let c1 = [p1[0] + d1[0] * s, p1[1] + d1[1] * s, p1[2] + d1[2] * s];
    let c2 = [p2[0] + d2[0] * t, p2[1] + d2[1] * t, p2[2] + d2[2] * t];
    let diff = sub(&c1, &c2);
    dot(&diff, &diff)
}

fn sub(a: &[f64; 3], b: &[f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Clamp a segment parameter into `[0, 1]`.
///
/// A NaN lands on 0 with everything else out of range below, and that is the safe direction: a
/// degenerate parameter puts the closest point at a segment endpoint, which is a real point on a
/// real capsule. `f64::clamp` would propagate the NaN into a distance, and a distance that compares
/// false against every threshold is a collision test that always passes.
fn clamp01(v: f64) -> f64 {
    if v > 1.0 {
        1.0
    } else if v > 0.0 {
        v
    } else {
        0.0
    }
}

/// A capsule's two endpoints, in the root frame.
pub fn capsule_in_world(pose: &Transform, capsule: &Capsule) -> ([f64; 3], [f64; 3]) {
    let a = rotate(&pose.rotation, &capsule.a);
    let b = rotate(&pose.rotation, &capsule.b);
    (
        [
            pose.translation[0] + a[0],
            pose.translation[1] + a[1],
            pose.translation[2] + a[2],
        ],
        [
            pose.translation[0] + b[0],
            pose.translation[1] + b[1],
            pose.translation[2] + b[2],
        ],
    )
}

/// Whether `a` lists `b`, or `b` lists `a`.
///
/// Symmetric on purpose. A body description that says the torso may touch the pelvis and forgets to
/// say the pelvis may touch the torso means the same thing, and a guard that read it as two
/// different claims would trip on the robot standing still.
pub fn ignores(frames: &[Frame<'_>], a: usize, b: usize) -> bool {
    frames[a].ignore.contains(&(b as u16)) || frames[b].ignore.contains(&(a as u16))
}

/// Whether one frame is the other's parent.
///
/// Excluded from the self-collision test unconditionally: two links joined by a joint touch at the
/// joint by construction, and no descriptor should have to declare that.
pub fn adjacent(frames: &[Frame<'_>], a: usize, b: usize) -> bool {
    frames[a].parent == Some(b as u16) || frames[b].parent == Some(a as u16)
}

/// Square root, re-exported so a consumer reporting a distance uses the specified one.
pub fn distance(squared: f64) -> f64 {
    sqrt(abs(squared))
}

#[cfg(test)]
mod tests;
