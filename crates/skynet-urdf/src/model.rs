//! The URDF, read into a structure that says what the file actually declared.
//!
//! Deliberately close to the file. This module does no interpretation at all — no role guessing,
//! no unit assumptions, no defaults invented for missing limits. Everything that turns a URDF into
//! an embodiment descriptor happens in [`super::descriptor`], and the split exists so that "what
//! the file says" and "what we concluded from it" are never the same value.
//!
//! # What it refuses, and why each refusal is worth its lines
//!
//! * **An unexpanded xacro.** Overwhelmingly the most common failure. `${...}` in an attribute or a
//!   `xacro:` element means the file is a template, and a template's joint limits are expressions
//!   rather than numbers. Parsing it leniently would produce a descriptor full of zeroes.
//! * **A joint naming a link that does not exist.** The frame graph would have a hole in it, and a
//!   hole in the frame graph is a `workspace` guard that cannot run (§44.5).
//! * **More than one root link.** Two disconnected trees is two robots, and a descriptor describes
//!   one body.
//! * **A cycle.** URDF is a tree by definition; a cycle means forward kinematics does not
//!   terminate.
//!
//! Every one names the fix, per §64.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::xml::{self, Element};

/// Joint kinds URDF defines.
///
/// `floating` and `planar` are parsed and then refused a role in the descriptor: they describe how
/// a body moves through the world rather than an actuator anyone commands, and the descriptor's
/// `base` field is where that fact belongs.
pub const URDF_JOINT_TYPES: [&str; 6] = [
    "revolute",
    "continuous",
    "prismatic",
    "fixed",
    "floating",
    "planar",
];

/// A joint, as declared.
#[derive(Debug, Clone, PartialEq)]
pub struct UrdfJoint {
    /// `name` attribute.
    pub name: String,
    /// One of [`URDF_JOINT_TYPES`].
    pub joint_type: String,
    /// `<parent link="…">`.
    pub parent: String,
    /// `<child link="…">`.
    pub child: String,
    /// `<origin xyz>`, metres. Absent in the file means the identity, which URDF states explicitly.
    pub origin_xyz: [f64; 3],
    /// `<origin rpy>`, radians, fixed-axis roll-pitch-yaw.
    pub origin_rpy: [f64; 3],
    /// `<axis xyz>`. URDF's default is `1 0 0`, and it applies to revolute, continuous and
    /// prismatic joints only.
    pub axis: [f64; 3],
    /// `<limit lower>`, radians or metres. `None` for a continuous joint, which has no travel
    /// bound by construction.
    pub limit_lower: Option<f64>,
    /// `<limit upper>`.
    pub limit_upper: Option<f64>,
    /// `<limit effort>`, Nm or N. **A design value from a CAD model, not a safety rating.**
    pub limit_effort: Option<f64>,
    /// `<limit velocity>`, rad/s or m/s. Same provenance caveat.
    pub limit_velocity: Option<f64>,
    /// `<mimic joint="…">` — this joint is driven by another one and is not independently
    /// commandable.
    pub mimic: Option<String>,
    /// `<mimic multiplier="…">`. URDF's default is 1.
    ///
    /// Carried because the commonest linkage in robotics is a parallel gripper whose second finger
    /// runs at −1, and a frame graph that could only record "follows" would put that finger on the
    /// wrong side of the object — inside the guard whose whole job is knowing where things are.
    pub mimic_multiplier: Option<f64>,
    /// `<mimic offset="…">`. URDF's default is 0.
    pub mimic_offset: Option<f64>,
}

impl UrdfJoint {
    /// Whether this joint moves at all.
    pub fn is_movable(&self) -> bool {
        matches!(
            self.joint_type.as_str(),
            "revolute" | "continuous" | "prismatic"
        )
    }

    /// Whether this joint is an independently commandable actuator.
    ///
    /// A mimic joint moves, and is not one: it is a linkage, and commanding it separately is how a
    /// parallel gripper gets torn apart. It appears in the frame graph and not in `joints[]`.
    pub fn is_actuator(&self) -> bool {
        self.is_movable() && self.mimic.is_none()
    }
}

/// A link, as declared. Only the parts a descriptor cares about are kept.
#[derive(Debug, Clone, PartialEq)]
pub struct UrdfLink {
    /// `name` attribute.
    pub name: String,
    /// Whether the link declares collision geometry, which is what §29's collision model needs.
    pub has_collision: bool,
    /// That geometry reduced to a sphere-swept segment, where it is one this kernel can evaluate.
    ///
    /// `None` alongside `has_collision: true` is a real and common link: the vendor's model is a
    /// mesh and nothing here will invent a bound for it. §44.3 keeps the two facts separate, and
    /// the `self_collision` guard reports itself unavailable naming those frames rather than
    /// testing a body with limbs missing from it.
    pub collision: Option<UrdfCapsule>,
    /// `<inertial><mass value>`, kilograms.
    pub mass_kg: Option<f64>,
}

/// A link's collision geometry, as the one primitive the kernel evaluates.
///
/// Every conversion below is **conservative**: the capsule contains the shape it stands for, so the
/// guard trips early rather than late. That is the only direction of error a collision bound is
/// allowed to have.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UrdfCapsule {
    /// One end of the segment, in the link frame.
    pub a: [f64; 3],
    /// The other end. Equal to `a` for a sphere.
    pub b: [f64; 3],
    /// Metres.
    pub radius_m: f64,
}

/// A parsed URDF.
#[derive(Debug, Clone, PartialEq)]
pub struct UrdfModel {
    /// `<robot name="…">`.
    pub name: String,
    /// Every link, in document order.
    pub links: Vec<UrdfLink>,
    /// Every joint, in document order.
    pub joints: Vec<UrdfJoint>,
    /// The single link no joint names as a child.
    pub root_link: String,
}

impl UrdfModel {
    /// Joints that are independently commandable actuators, in document order.
    pub fn actuators(&self) -> impl Iterator<Item = &UrdfJoint> {
        self.joints.iter().filter(|joint| joint.is_actuator())
    }

    /// The joint whose child is this link, if any.
    pub fn joint_to(&self, link: &str) -> Option<&UrdfJoint> {
        self.joints.iter().find(|joint| joint.child == link)
    }

    /// Links in a stable depth-first order from the root.
    ///
    /// Depth-first from the root rather than document order, because the frame graph is consumed as
    /// a tree and a parent must be defined before its children for a single-pass reader to build it.
    pub fn links_from_root(&self) -> Vec<&UrdfLink> {
        let mut out: Vec<&UrdfLink> = Vec::new();
        let mut stack: Vec<&str> = Vec::new();
        stack.push(&self.root_link);
        while let Some(current) = stack.pop() {
            if let Some(link) = self.links.iter().find(|link| link.name == current) {
                out.push(link);
            }
            // Reversed so document order survives the stack.
            let children: Vec<&str> = self
                .joints
                .iter()
                .filter(|joint| joint.parent == current)
                .map(|joint| joint.child.as_str())
                .collect();
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        }
        out
    }
}

/// Why a URDF could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrdfError {
    /// What is wrong and what to do about it.
    pub message: String,
    /// Line, when the failure came from the XML layer.
    pub line: Option<usize>,
    /// Column, when the failure came from the XML layer.
    pub column: Option<usize>,
}

impl UrdfError {
    fn at(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
        }
    }
}

impl fmt::Display for UrdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(f, "line {line}, column {column}: {}", self.message)
            }
            _ => write!(f, "{}", self.message),
        }
    }
}

impl From<xml::XmlError> for UrdfError {
    fn from(err: xml::XmlError) -> Self {
        Self {
            message: err.message,
            line: Some(err.line),
            column: Some(err.column),
        }
    }
}

/// Read a URDF.
pub fn parse(input: &str) -> Result<UrdfModel, UrdfError> {
    let root = xml::parse(input)?;

    if root.name != "robot" {
        return Err(UrdfError::at(format!(
            "the root element is `<{}>`, not `<robot>`. This is not a URDF — an SRDF, a MJCF scene \
             or a launch file all parse as XML and none of them describes a body.",
            root.name
        )));
    }

    refuse_unexpanded_xacro(&root)?;

    let name = root.attr("name").unwrap_or("").to_string();

    let mut links: Vec<UrdfLink> = Vec::new();
    for element in root.children_named("link") {
        let link_name = required_attr(element, "name", "link")?;
        if links.iter().any(|existing| existing.name == link_name) {
            return Err(UrdfError::at(format!(
                "two links are named `{link_name}`. Link names are the frame graph's identifiers \
                 and must be unique."
            )));
        }
        let collision_element = element.child("collision");
        links.push(UrdfLink {
            has_collision: collision_element.is_some(),
            collision: collision_element.and_then(collision_capsule),
            mass_kg: element
                .child("inertial")
                .and_then(|inertial| inertial.child("mass"))
                .and_then(|mass| mass.attr("value"))
                .and_then(|value| parse_number(value).ok()),
            name: link_name,
        });
    }

    let mut joints: Vec<UrdfJoint> = Vec::new();
    for element in root.children_named("joint") {
        joints.push(parse_joint(element)?);
    }

    for joint in &joints {
        if joints
            .iter()
            .filter(|other| other.name == joint.name)
            .count()
            > 1
        {
            return Err(UrdfError::at(format!(
                "two joints are named `{}`. Joint names are what a command addresses; two of them \
                 is an ambiguous command.",
                joint.name
            )));
        }
    }

    check_link_references(&links, &joints)?;
    let root_link = find_root_link(&links, &joints)?;

    Ok(UrdfModel {
        name,
        links,
        joints,
        root_link,
    })
}

/// The refusal that saves the most time, because the file looks fine until you read the numbers.
fn refuse_unexpanded_xacro(root: &Element) -> Result<(), UrdfError> {
    fn has_xacro(element: &Element) -> Option<String> {
        if element.name.starts_with("xacro:") {
            return Some(format!("the element `<{}>`", element.name));
        }
        for (key, value) in &element.attributes {
            // `xmlns:xacro` deliberately does not count. A file that declares the namespace and
            // contains no template construct really has been expanded, and refusing it would be a
            // refusal of a file that is fine — which teaches people to distrust the check.
            if key.starts_with("xacro:") {
                return Some(format!("the attribute `{key}`"));
            }
            // `${…}` only. `$(find pkg)` also means a preprocessor has not run, but it turns up in
            // mesh filenames — which feed nothing in the descriptor — and refusing a file this tool
            // could read correctly is worse than missing one. A `$(arg …)` where a number belongs
            // is caught by the number parser instead, which names the joint and the attribute.
            if value.contains("${") {
                return Some(format!("`{key}=\"{value}\"`, which is an expression"));
            }
        }
        element.children.iter().find_map(has_xacro)
    }

    if let Some(evidence) = has_xacro(root) {
        return Err(UrdfError::at(format!(
            "this file is a xacro template, not a URDF — {evidence}. Expand it first:\n    \
             xacro robot.urdf.xacro > robot.urdf\nA template's joint limits are expressions, and \
             reading them as numbers would put zeroes into a safety-relevant field."
        )));
    }
    Ok(())
}

fn parse_joint(element: &Element) -> Result<UrdfJoint, UrdfError> {
    let name = required_attr(element, "name", "joint")?;
    let joint_type = element.attr("type").unwrap_or("").to_string();

    if !URDF_JOINT_TYPES.contains(&joint_type.as_str()) {
        return Err(UrdfError::at(if joint_type.is_empty() {
            format!(
                "joint `{name}` declares no `type`. URDF requires one of: {}.",
                URDF_JOINT_TYPES.join(", ")
            )
        } else {
            format!(
                "joint `{name}` has type `{joint_type}`, which URDF does not define. Valid types: {}.",
                URDF_JOINT_TYPES.join(", ")
            )
        }));
    }

    let parent = element
        .child("parent")
        .and_then(|node| node.attr("link"))
        .map(str::to_string)
        .ok_or_else(|| {
            UrdfError::at(format!(
                "joint `{name}` has no `<parent link=\"…\"/>`. Every joint connects two links."
            ))
        })?;
    let child = element
        .child("child")
        .and_then(|node| node.attr("link"))
        .map(str::to_string)
        .ok_or_else(|| {
            UrdfError::at(format!(
                "joint `{name}` has no `<child link=\"…\"/>`. Every joint connects two links."
            ))
        })?;

    if parent == child {
        return Err(UrdfError::at(format!(
            "joint `{name}` has `{parent}` as both parent and child, which is a self-loop rather \
             than a connection."
        )));
    }

    let origin = element.child("origin");
    let origin_xyz = vector3(
        origin.and_then(|node| node.attr("xyz")),
        [0.0; 3],
        &name,
        "origin xyz",
    )?;
    let origin_rpy = vector3(
        origin.and_then(|node| node.attr("rpy")),
        [0.0; 3],
        &name,
        "origin rpy",
    )?;
    // URDF's stated default axis. It only means anything for a joint that moves.
    let axis = vector3(
        element.child("axis").and_then(|node| node.attr("xyz")),
        [1.0, 0.0, 0.0],
        &name,
        "axis xyz",
    )?;

    let limit = element.child("limit");
    let number = |attribute: &str| -> Result<Option<f64>, UrdfError> {
        match limit.and_then(|node| node.attr(attribute)) {
            None => Ok(None),
            Some(raw) => parse_number(raw).map(Some).map_err(|_| {
                UrdfError::at(format!(
                    "joint `{name}` has `{attribute}=\"{raw}\"`, which is not a number."
                ))
            }),
        }
    };

    let joint = UrdfJoint {
        joint_type,
        parent,
        child,
        origin_xyz,
        origin_rpy,
        axis,
        limit_lower: number("lower")?,
        limit_upper: number("upper")?,
        limit_effort: number("effort")?,
        limit_velocity: number("velocity")?,
        mimic: element
            .child("mimic")
            .and_then(|node| node.attr("joint"))
            .map(str::to_string),
        mimic_multiplier: element
            .child("mimic")
            .and_then(|node| node.attr("multiplier"))
            .and_then(|raw| parse_number(raw).ok()),
        mimic_offset: element
            .child("mimic")
            .and_then(|node| node.attr("offset"))
            .and_then(|raw| parse_number(raw).ok()),
        name,
    };

    if let (Some(lower), Some(upper)) = (joint.limit_lower, joint.limit_upper) {
        if lower > upper {
            return Err(UrdfError::at(format!(
                "joint `{}` has lower={lower} above upper={upper}. A travel range that excludes \
                 every position would clamp every command to nothing.",
                joint.name
            )));
        }
    }

    Ok(joint)
}

fn check_link_references(links: &[UrdfLink], joints: &[UrdfJoint]) -> Result<(), UrdfError> {
    let known = |name: &str| links.iter().any(|link| link.name == name);
    for joint in joints {
        for (role, link) in [("parent", &joint.parent), ("child", &joint.child)] {
            if !known(link) {
                return Err(UrdfError::at(format!(
                    "joint `{}` names `{link}` as its {role}, and no `<link name=\"{link}\">` is \
                     declared. Either the link is missing or the name is misspelt; the frame graph \
                     cannot be built with a hole in it.",
                    joint.name
                )));
            }
        }
    }
    Ok(())
}

fn find_root_link(links: &[UrdfLink], joints: &[UrdfJoint]) -> Result<String, UrdfError> {
    if links.is_empty() {
        return Err(UrdfError::at(
            "the file declares no `<link>`. A robot with no links has no body to describe.",
        ));
    }

    // A link with two parent joints is a closed chain, which URDF cannot express and forward
    // kinematics cannot resolve without an explicit loop closure.
    for link in links {
        let parents = joints
            .iter()
            .filter(|joint| joint.child == link.name)
            .count();
        if parents > 1 {
            return Err(UrdfError::at(format!(
                "link `{}` is the child of {parents} joints. URDF describes a tree; a link with two \
                 parents is a closed kinematic chain, which needs a loop-closure model this format \
                 has no way to express.",
                link.name
            )));
        }
    }

    let roots: Vec<&str> = links
        .iter()
        .filter(|link| !joints.iter().any(|joint| joint.child == link.name))
        .map(|link| link.name.as_str())
        .collect();

    match roots.len() {
        1 => Ok(roots[0].to_string()),
        0 => Err(UrdfError::at(
            "every link has a parent joint, so the link tree contains a cycle. URDF describes a \
             tree, and forward kinematics over a cycle does not terminate.",
        )),
        _ => Err(UrdfError::at(format!(
            "{} links have no parent joint: {}. A descriptor describes one body, and these are \
             {} disconnected trees. Join them with a `fixed` joint, or split the file.",
            roots.len(),
            roots.join(", "),
            roots.len()
        ))),
    }
}

/// Reduce a `<collision>` element to a capsule, or decline.
///
/// URDF offers four geometries and this handles three of them:
///
/// * **`<sphere radius>`** is a segment of no length. Exact.
/// * **`<cylinder radius length>`** is the primitive itself: a segment along the link frame's z,
///   which is the axis URDF gives a cylinder. A capsule of the same radius contains the cylinder
///   and adds two hemispherical caps, so it is conservative by two half-spheres.
/// * **`<box size>`** becomes a segment along its longest axis, with a radius equal to half the
///   diagonal of the cross-section perpendicular to it. That is the tightest capsule that certainly
///   contains the box, and it is loose — for a cube it is the circumscribed sphere. A body modelled
///   entirely in boxes will trip this guard sooner than its geometry requires, which is the error
///   in the direction a safety bound is allowed to make.
/// * **`<mesh>`** produces nothing. Bounding a mesh needs the mesh, which is a file this importer
///   does not read and may not have; a guessed bound around a hand is a guard that either trips
///   constantly or misses the hand entirely, and both are worse than saying so.
///
/// The `<origin>` offset is applied; its `rpy` is **not**, and that is a limitation worth naming: a
/// rotated cylinder is bounded here as though it pointed along z. §53 carries it as open.
fn collision_capsule(element: &Element) -> Option<UrdfCapsule> {
    let geometry = element.child("geometry")?;
    let origin = element.child("origin");
    let centre = vector3(origin.and_then(|node| node.attr("xyz")), [0.0; 3], "", "").ok()?;
    let offset = |v: [f64; 3]| [centre[0] + v[0], centre[1] + v[1], centre[2] + v[2]];

    if let Some(sphere) = geometry.child("sphere") {
        let radius = parse_number(sphere.attr("radius")?).ok()?;
        if !(radius.is_finite() && radius > 0.0) {
            return None;
        }
        return Some(UrdfCapsule {
            a: centre,
            b: centre,
            radius_m: radius,
        });
    }

    if let Some(cylinder) = geometry.child("cylinder") {
        let radius = parse_number(cylinder.attr("radius")?).ok()?;
        let length = parse_number(cylinder.attr("length")?).ok()?;
        if !(radius.is_finite() && radius > 0.0 && length.is_finite() && length >= 0.0) {
            return None;
        }
        let half = 0.5 * length;
        return Some(UrdfCapsule {
            a: offset([0.0, 0.0, -half]),
            b: offset([0.0, 0.0, half]),
            radius_m: radius,
        });
    }

    if let Some(shape) = geometry.child("box") {
        let size = vector3(shape.attr("size"), [0.0; 3], "", "").ok()?;
        if size.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return None;
        }
        // The longest axis carries the segment; the other two set the radius.
        let mut longest = 0;
        for (axis, extent) in size.iter().enumerate() {
            if *extent > size[longest] {
                longest = axis;
            }
        }
        let mut cross = [0.0f64; 2];
        let mut slot = 0;
        for (axis, extent) in size.iter().enumerate() {
            if axis != longest {
                cross[slot] = *extent;
                slot += 1;
            }
        }
        let radius = 0.5 * (cross[0] * cross[0] + cross[1] * cross[1]).sqrt();
        if !(radius.is_finite() && radius > 0.0) {
            return None;
        }
        let half = 0.5 * size[longest];
        let mut ends = [[0.0f64; 3]; 2];
        ends[0][longest] = -half;
        ends[1][longest] = half;
        return Some(UrdfCapsule {
            a: offset(ends[0]),
            b: offset(ends[1]),
            radius_m: radius,
        });
    }

    None
}

fn required_attr(element: &Element, attribute: &str, kind: &str) -> Result<String, UrdfError> {
    element
        .attr(attribute)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            UrdfError::at(format!(
                "a `<{kind}>` has no `{attribute}`. Every {kind} needs one; it is how everything \
                 else refers to it."
            ))
        })
}

fn vector3(
    raw: Option<&str>,
    default: [f64; 3],
    joint: &str,
    field: &str,
) -> Result<[f64; 3], UrdfError> {
    let Some(raw) = raw else { return Ok(default) };
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(UrdfError::at(format!(
            "joint `{joint}` has `{field}` = \"{raw}\", which is {} values rather than 3.",
            parts.len()
        )));
    }
    let mut out = [0.0f64; 3];
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = parse_number(part).map_err(|_| {
            UrdfError::at(format!(
                "joint `{joint}` has `{field}` = \"{raw}\", and `{part}` is not a number."
            ))
        })?;
    }
    Ok(out)
}

/// Parse a decimal, refusing the non-finite spellings.
///
/// `inf` and `nan` parse happily in Rust and are exactly the values that must never reach a joint
/// limit: a `NaN` upper bound compares false against everything, so a clamp built from it silently
/// passes every command through.
pub(crate) fn parse_number(raw: &str) -> Result<f64, ()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    match trimmed.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_JOINT_ARM: &str = r#"
<robot name="two joint arm">
  <link name="base_link"><collision/></link>
  <link name="link1"/>
  <link name="tool"/>
  <joint name="shoulder_pitch" type="revolute">
    <parent link="base_link"/><child link="link1"/>
    <origin xyz="0 0 0.1" rpy="0 0 0"/>
    <axis xyz="0 1 0"/>
    <limit lower="-1.57" upper="1.57" effort="80" velocity="2.0"/>
  </joint>
  <joint name="tool_mount" type="fixed">
    <parent link="link1"/><child link="tool"/>
    <origin xyz="0 0 0.3"/>
  </joint>
</robot>"#;

    #[test]
    fn a_urdf_reads_as_what_it_declared() {
        let model = parse(TWO_JOINT_ARM).unwrap();
        assert_eq!(model.name, "two joint arm");
        assert_eq!(model.root_link, "base_link");
        assert_eq!(model.links.len(), 3);
        assert_eq!(model.joints.len(), 2);

        let shoulder = &model.joints[0];
        assert_eq!(shoulder.joint_type, "revolute");
        assert_eq!(shoulder.axis, [0.0, 1.0, 0.0]);
        assert_eq!(shoulder.origin_xyz, [0.0, 0.0, 0.1]);
        assert_eq!(shoulder.limit_effort, Some(80.0));
        assert_eq!(shoulder.limit_velocity, Some(2.0));

        assert_eq!(model.actuators().count(), 1);
        assert!(model.links[0].has_collision);
        assert!(!model.links[1].has_collision);
    }

    /// The default axis is URDF's, not ours, and a joint that omits `<axis>` really does rotate
    /// about x. Defaulting it to zero would produce a frame graph that never moves.
    #[test]
    fn the_urdf_defaults_are_the_urdf_defaults() {
        let model = parse(
            r#"<robot name="d"><link name="a"/><link name="b"/>
               <joint name="j" type="continuous"><parent link="a"/><child link="b"/></joint></robot>"#,
        )
        .unwrap();
        assert_eq!(model.joints[0].axis, [1.0, 0.0, 0.0]);
        assert_eq!(model.joints[0].origin_xyz, [0.0; 3]);
        assert_eq!(model.joints[0].limit_lower, None);
    }

    /// A mimic joint moves and is not an actuator. Commanding a parallel gripper's mirrored finger
    /// separately is how the linkage gets torn apart, so it never reaches `joints[]`.
    #[test]
    fn a_mimic_joint_is_not_an_actuator() {
        let model = parse(
            r#"<robot name="g"><link name="a"/><link name="b"/><link name="c"/>
               <joint name="finger_left" type="prismatic"><parent link="a"/><child link="b"/>
                 <limit lower="0" upper="0.04" effort="20" velocity="0.1"/></joint>
               <joint name="finger_right" type="prismatic"><parent link="a"/><child link="c"/>
                 <mimic joint="finger_left" multiplier="-1"/>
                 <limit lower="-0.04" upper="0" effort="20" velocity="0.1"/></joint>
               </robot>"#,
        )
        .unwrap();
        assert_eq!(model.joints.len(), 2);
        assert_eq!(model.actuators().count(), 1);
        assert_eq!(model.actuators().next().unwrap().name, "finger_left");
        assert!(model.joints[1].is_movable());
        assert!(!model.joints[1].is_actuator());
    }

    #[test]
    fn an_unexpanded_xacro_is_refused_by_name() {
        let err = parse(
            r#"<robot name="r" xmlns:xacro="http://ros.org/wiki/xacro">
                 <link name="a"/><xacro:property name="w" value="1"/></robot>"#,
        )
        .unwrap_err();
        assert!(err.message.contains("xacro template"), "{}", err.message);
        assert!(
            err.message.contains("xacro robot.urdf.xacro"),
            "{}",
            err.message
        );

        let err = parse(
            r#"<robot name="r"><link name="a"/><link name="b"/>
               <joint name="j" type="revolute"><parent link="a"/><child link="b"/>
                 <limit lower="${-pi/2}" upper="1" effort="1" velocity="1"/></joint></robot>"#,
        )
        .unwrap_err();
        assert!(err.message.contains("is an expression"), "{}", err.message);
    }

    #[test]
    fn structural_failures_name_the_fix() {
        let err = parse(r#"<srdf name="x"/>"#).unwrap_err();
        assert!(err.message.contains("not a URDF"), "{}", err.message);

        let err = parse(
            r#"<robot name="r"><link name="a"/>
               <joint name="j" type="fixed"><parent link="a"/><child link="typo"/></joint></robot>"#,
        )
        .unwrap_err();
        assert!(err.message.contains("`typo`"), "{}", err.message);
        assert!(err.message.contains("misspelt"), "{}", err.message);

        let err = parse(r#"<robot name="r"><link name="a"/><link name="b"/></robot>"#).unwrap_err();
        assert!(
            err.message.contains("disconnected trees"),
            "{}",
            err.message
        );

        let err = parse(
            r#"<robot name="r"><link name="a"/><link name="b"/>
               <joint name="j" type="screw"><parent link="a"/><child link="b"/></joint></robot>"#,
        )
        .unwrap_err();
        assert!(err.message.contains("`screw`"), "{}", err.message);
        assert!(err.message.contains("revolute"), "{}", err.message);
    }

    /// A closed chain is a real robot — a delta, a four-bar gripper — that URDF cannot express, and
    /// the failure must say that rather than "invalid file".
    #[test]
    fn a_closed_chain_is_named_as_one() {
        let err = parse(
            r#"<robot name="r"><link name="a"/><link name="b"/><link name="c"/>
               <joint name="j1" type="fixed"><parent link="a"/><child link="c"/></joint>
               <joint name="j2" type="fixed"><parent link="b"/><child link="c"/></joint></robot>"#,
        )
        .unwrap_err();
        assert!(
            err.message.contains("closed kinematic chain"),
            "{}",
            err.message
        );
    }

    /// `inf` and `nan` parse in Rust and must not reach a joint limit: a NaN bound compares false
    /// against everything, so a clamp built from it passes every command through.
    #[test]
    fn non_finite_limits_are_refused() {
        for spelling in ["inf", "-inf", "NaN", "infinity"] {
            let err = parse(&format!(
                r#"<robot name="r"><link name="a"/><link name="b"/>
                   <joint name="j" type="revolute"><parent link="a"/><child link="b"/>
                     <limit lower="-1" upper="1" effort="{spelling}" velocity="1"/></joint></robot>"#
            ))
            .unwrap_err();
            assert!(
                err.message.contains("not a number"),
                "{spelling}: {}",
                err.message
            );
        }
    }

    #[test]
    fn an_inverted_travel_range_is_refused() {
        let err = parse(
            r#"<robot name="r"><link name="a"/><link name="b"/>
               <joint name="j" type="revolute"><parent link="a"/><child link="b"/>
                 <limit lower="2" upper="-2" effort="1" velocity="1"/></joint></robot>"#,
        )
        .unwrap_err();
        assert!(
            err.message.contains("excludes every position"),
            "{}",
            err.message
        );
    }

    #[test]
    fn links_come_back_depth_first_from_the_root() {
        let model = parse(TWO_JOINT_ARM).unwrap();
        let order: Vec<&str> = model
            .links_from_root()
            .iter()
            .map(|link| link.name.as_str())
            .collect();
        assert_eq!(order, ["base_link", "link1", "tool"]);
    }
}

#[cfg(test)]
mod collision_tests {
    use super::*;

    fn link(xml: &str) -> UrdfLink {
        let model = parse(&format!(
            "<robot name=\"r\"><link name=\"base_link\">{xml}</link></robot>"
        ))
        .expect("parses");
        model.links[0].clone()
    }

    #[test]
    fn a_sphere_is_a_segment_of_no_length() {
        let capsule = link("<collision><geometry><sphere radius=\"0.12\"/></geometry></collision>")
            .collision
            .expect("a sphere converts exactly");
        assert_eq!(capsule.a, capsule.b);
        assert_eq!(capsule.radius_m, 0.12);
    }

    #[test]
    fn a_cylinder_becomes_a_capsule_about_its_own_axis() {
        // URDF puts a cylinder's length along z. A capsule of the same radius contains it and adds
        // two hemispherical caps, which is conservative — the direction of error a bound may have.
        let capsule =
            link("<collision><geometry><cylinder radius=\"0.05\" length=\"0.4\"/></geometry></collision>")
                .collision
                .expect("a cylinder converts");
        assert_eq!(capsule.a, [0.0, 0.0, -0.2]);
        assert_eq!(capsule.b, [0.0, 0.0, 0.2]);
        assert_eq!(capsule.radius_m, 0.05);
    }

    #[test]
    fn a_box_takes_its_segment_along_its_longest_axis() {
        let capsule =
            link("<collision><geometry><box size=\"0.1 0.6 0.2\"/></geometry></collision>")
                .collision
                .expect("a box converts");
        assert_eq!(capsule.a, [0.0, -0.3, 0.0]);
        assert_eq!(capsule.b, [0.0, 0.3, 0.0]);
        // Half the diagonal of the 0.1 × 0.2 cross-section.
        let expected = 0.5 * (0.1f64 * 0.1 + 0.2 * 0.2).sqrt();
        assert!((capsule.radius_m - expected).abs() < 1e-12);
    }

    #[test]
    fn the_origin_offset_moves_the_capsule() {
        let capsule = link(
            "<collision><origin xyz=\"0 0 0.5\"/><geometry><sphere radius=\"0.1\"/></geometry></collision>",
        )
        .collision
        .expect("converts");
        assert_eq!(capsule.a, [0.0, 0.0, 0.5]);
    }

    #[test]
    fn a_mesh_declares_geometry_and_no_shape() {
        // Two different facts, and §44.3 keeps them apart: there is collision geometry here, and
        // this kernel cannot evaluate it. Inventing a bound around a hand would produce a guard
        // that either trips constantly or misses the hand, and both are worse than saying so.
        let link =
            link("<collision><geometry><mesh filename=\"hand.stl\"/></geometry></collision>");
        assert!(link.has_collision);
        assert!(link.collision.is_none());
    }

    #[test]
    fn a_sphere_of_no_radius_is_declined_rather_than_emitted() {
        let link = link("<collision><geometry><sphere radius=\"0\"/></geometry></collision>");
        assert!(link.has_collision);
        assert!(
            link.collision.is_none(),
            "a capsule of no radius bounds nothing"
        );
    }

    #[test]
    fn a_mimic_carries_its_ratio() {
        let model = parse(
            "<robot name=\"r\">\
               <link name=\"base_link\"/><link name=\"a\"/><link name=\"b\"/>\
               <joint name=\"finger_left\" type=\"prismatic\">\
                 <parent link=\"base_link\"/><child link=\"a\"/>\
               </joint>\
               <joint name=\"finger_right\" type=\"prismatic\">\
                 <parent link=\"base_link\"/><child link=\"b\"/>\
                 <mimic joint=\"finger_left\" multiplier=\"-1\" offset=\"0.002\"/>\
               </joint>\
             </robot>",
        )
        .expect("parses");
        let right = model
            .joints
            .iter()
            .find(|joint| joint.name == "finger_right")
            .unwrap();
        assert_eq!(right.mimic.as_deref(), Some("finger_left"));
        assert_eq!(right.mimic_multiplier, Some(-1.0));
        assert_eq!(right.mimic_offset, Some(0.002));
    }
}
