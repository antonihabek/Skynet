//! Turning a descriptor's frame graph into the shape the Safety Kernel reads.
//!
//! Two shapes, one tree, and the gap between them is deliberate.
//!
//! §44.3's graph is a list of nodes naming their parents by string, in no required order. That is
//! right for a signed document that a human reads, two generators write and a schema validates.
//! [`skynet_kernel::kinematics::FrameSet`] is a slice of frames carrying **parent indices strictly
//! smaller than their own**, which is right for a control cycle: forward kinematics becomes one
//! forward pass, and a cycle in the graph becomes unrepresentable rather than a hang.
//!
//! This module is the one place that converts between them. It runs **once**, when a lease is
//! granted — never in a cycle — which is what licenses it to allocate, sort, and do the O(n²) name
//! resolution the kernel must not.
//!
//! # Why it can fail
//!
//! Because a descriptor can be wrong, and every way it can be wrong is better discovered here than
//! in a control loop. [`flatten`] runs [`validate_frames`] first and then adds the two checks that
//! only matter once indices exist: that the tree is small enough to fit the kernel's bound, and
//! that a topological order exists at all. Both return a [`FrameFault`] naming frames, because
//! §44.5 requires the guards to report with the missing frame named and "the graph is malformed" is
//! not a sentence an operator can act on.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_kernel::kinematics::{
    Capsule, Frame, FrameSet, Joint, JointKind, Mimic, Transform, MAX_FRAMES,
};

use crate::embodiment::{validate_frames, FrameFault, FrameGraph};

/// A frame graph, flattened and owned.
///
/// Holds the strings and the index lists the kernel's borrowed view points at. One of these lives
/// for the length of a lease; [`Self::frames`] builds the borrowed slice, and
/// `FrameSet::new` over it is what the kernel actually reads.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatFrames {
    entries: Vec<FlatFrame>,
}

/// One frame, owned.
#[derive(Debug, Clone, PartialEq)]
struct FlatFrame {
    name: String,
    parent: Option<u16>,
    translation: [f64; 3],
    rotation: [f64; 4],
    joint: Option<FlatJoint>,
    collision: Option<Capsule>,
    ignore: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq)]
struct FlatJoint {
    name: String,
    kind: JointKind,
    axis: [f64; 3],
    mimic: Option<FlatMimic>,
}

#[derive(Debug, Clone, PartialEq)]
struct FlatMimic {
    joint: String,
    multiplier: f64,
    offset: f64,
}

fn fault(code: &str, subjects: &[&str], detail: String) -> FrameFault {
    FrameFault {
        code: code.to_string(),
        subjects: subjects.iter().map(|s| String::from(*s)).collect(),
        detail,
    }
}

impl FlatFrames {
    /// The borrowed view the kernel reads.
    ///
    /// A `Vec` rather than a slice because the borrow has to be materialised somewhere, and this is
    /// the same ceremony `LimitsOwned` performs for the channel map: the caller holds it for the
    /// length of the lease, and nothing in the cycle allocates.
    pub fn frames(&self) -> Vec<Frame<'_>> {
        self.entries
            .iter()
            .map(|entry| Frame {
                name: entry.name.as_str(),
                parent: entry.parent,
                local: Transform {
                    translation: entry.translation,
                    rotation: entry.rotation,
                },
                joint: entry.joint.as_ref().map(|joint| Joint {
                    name: joint.name.as_str(),
                    kind: joint.kind,
                    axis: joint.axis,
                    mimic: joint.mimic.as_ref().map(|mimic| Mimic {
                        joint: mimic.joint.as_str(),
                        multiplier: mimic.multiplier,
                        offset: mimic.offset,
                    }),
                }),
                collision: entry.collision,
                ignore: entry.ignore.as_slice(),
            })
            .collect()
    }

    /// How many frames.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Flatten a §44.3 frame graph into the kernel's order.
///
/// Breadth-first from the root, so a parent is always emitted before its children and the ordering
/// invariant holds by construction rather than by a sort that has to be checked afterwards.
///
/// The `frames` in the descriptor may be in any order; the output's order is a function of the tree
/// and of the descriptor's own node order among siblings, which makes it stable across editions —
/// a requirement, because the kernel's verdicts name frames and a corpus compares those names in
/// order.
pub fn flatten(graph: &FrameGraph) -> Result<FlatFrames, FrameFault> {
    if let Some(fault) = validate_frames(graph) {
        return Err(fault);
    }
    if graph.nodes.len() > MAX_FRAMES {
        return Err(fault(
            "too_large",
            &[],
            format!(
                "The body declares {} frames and this kernel is built for {MAX_FRAMES}. Half a \
                 frame graph is a robot with a limb missing from its own collision model, so the \
                 graph is refused rather than truncated.",
                graph.nodes.len()
            ),
        ));
    }

    // Breadth-first from the declared root. `validate_frames` has already established that there is
    // exactly one root, that every parent exists and that there is no cycle, so the only way a node
    // can be missed here is if it is not connected to the declared root — which validation permits
    // and this does not.
    let mut order: Vec<usize> = Vec::with_capacity(graph.nodes.len());
    let root = graph
        .nodes
        .iter()
        .position(|node| node.name == graph.root)
        .ok_or_else(|| {
            fault(
                "root_missing",
                &[graph.root.as_str()],
                format!("The declared root {} is not one of the frames.", graph.root),
            )
        })?;
    order.push(root);
    let mut cursor = 0;
    while cursor < order.len() {
        let parent = &graph.nodes[order[cursor]];
        for (index, node) in graph.nodes.iter().enumerate() {
            if node.parent.as_deref() == Some(parent.name.as_str()) {
                order.push(index);
            }
        }
        cursor += 1;
    }

    if order.len() != graph.nodes.len() {
        let orphan = graph
            .nodes
            .iter()
            .enumerate()
            .find(|(index, _)| !order.contains(index))
            .map(|(_, node)| node.name.as_str())
            .unwrap_or("");
        return Err(fault(
            "unreachable",
            &[orphan],
            format!(
                "Frame {orphan} is not reachable from the root {}. Its parent chain is valid and \
                 leads somewhere else, which is a second tree in one document.",
                graph.root
            ),
        ));
    }

    // Descriptor index → position in the flattened order, for resolving parents and ignore lists.
    let mut position: Vec<u16> = alloc::vec![0; graph.nodes.len()];
    for (slot, index) in order.iter().enumerate() {
        position[*index] = slot as u16;
    }

    let mut entries: Vec<FlatFrame> = Vec::with_capacity(order.len());
    for index in &order {
        let node = &graph.nodes[*index];
        let parent = match &node.parent {
            None => None,
            Some(parent) => graph
                .nodes
                .iter()
                .position(|other| &other.name == parent)
                .map(|found| position[found]),
        };

        let joint = match (&node.joint, &node.joint_type) {
            (Some(name), Some(kind)) => {
                let kind = JointKind::parse(kind).ok_or_else(|| {
                    fault(
                        "unknown_joint_type",
                        &[node.name.as_str()],
                        format!(
                            "Frame {} names joint type {kind}, which is not one of the five §44.3 \
                             admits. Reading it as revolute is how a prismatic axis becomes a \
                             rotation.",
                            node.name
                        ),
                    )
                })?;
                Some(FlatJoint {
                    name: name.clone(),
                    kind,
                    // URDF's default, stated: the axis is `1 0 0` when the file is silent.
                    axis: node.axis.unwrap_or([1.0, 0.0, 0.0]),
                    mimic: node.mimics.as_ref().map(|driver| FlatMimic {
                        joint: driver.clone(),
                        multiplier: node.mimic_multiplier.unwrap_or(1.0),
                        offset: node.mimic_offset.unwrap_or(0.0),
                    }),
                })
            }
            // `validate_frames` has already refused a dynamic frame with no joint and a static one
            // with a joint, so what remains here is a joint named without a type — which the
            // kernel cannot act on and will not guess at.
            (Some(_), None) => {
                return Err(fault(
                    "unknown_joint_type",
                    &[node.name.as_str()],
                    format!(
                        "Frame {} names a joint and no joint type, so nothing can tell whether it \
                         turns or slides.",
                        node.name
                    ),
                ))
            }
            _ => None,
        };

        let mut ignore: Vec<u16> = Vec::new();
        for name in &node.collision_ignore {
            if let Some(found) = graph.nodes.iter().position(|other| &other.name == name) {
                ignore.push(position[found]);
            }
        }
        ignore.sort_unstable();

        entries.push(FlatFrame {
            name: node.name.clone(),
            parent,
            translation: node.translation.unwrap_or([0.0, 0.0, 0.0]),
            // Absent means no rotation, and the identity quaternion is `xyzw = 0 0 0 1` — not
            // `0 0 0 0`, which is not a rotation at all and would collapse every pose below it.
            rotation: node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]),
            joint,
            collision: node.collision_shape.as_ref().map(|shape| Capsule {
                a: shape.a,
                b: shape.b,
                radius_m: shape.radius_m,
            }),
            ignore,
        });
    }

    Ok(FlatFrames { entries })
}

/// Flatten and then hand the result to a closure holding the kernel's view.
///
/// The borrow chain is three deep — owned frames, a borrowed slice, a checked set — and every
/// caller would otherwise write the same three lines and the same `expect`. The closure form makes
/// the lifetimes fall out and keeps [`FrameSet::new`]'s refusal in one place.
pub fn with_frame_set<T>(
    graph: &FrameGraph,
    body: impl FnOnce(FrameSet<'_>) -> T,
) -> Result<T, FrameFault> {
    let flat = flatten(graph)?;
    let frames = flat.frames();
    let set = FrameSet::new(&frames).map_err(|error| {
        fault(
            error.as_str(),
            &[],
            format!(
                "The flattened frame set was refused: {}. This is a defect in the flattening \
                 rather than in the descriptor, which passed validation.",
                error.as_str()
            ),
        )
    })?;
    Ok(body(set))
}

// ── The wire shapes, for the corpus and for a surface ────────────────────────

use skynet_json::Json;
use skynet_kernel::kinematics::{Kinematics, Shape, Volume, WorkspaceLayer};

/// Serialise a solved pose set.
///
/// The corpus pins this, which is the point: it holds two editions to the same *arithmetic* and not
/// only to the same verdicts. A trip is a boolean and would agree across two implementations that
/// disagreed about every pose by a millimetre; a pose is thirty digits and does not.
pub fn solve_to_json(set: &FrameSet<'_>, solved: &Kinematics) -> Json {
    let mut frames = Json::arr();
    for (index, frame) in set.frames().iter().enumerate() {
        let mut item = Json::obj();
        item.set("name", Json::from(frame.name));
        match solved.pose(index) {
            None => {
                item.set("resolved", Json::Bool(false));
                // Present and null, not absent. "This frame has no pose" is the fact the guards act
                // on, and a consumer reading an absent key cannot tell it from a truncated document.
                item.set("translation", Json::Null);
                item.set("rotation", Json::Null);
            }
            Some(pose) => {
                item.set("resolved", Json::Bool(true));
                item.set("translation", numbers(&pose.translation));
                item.set("rotation", numbers(&pose.rotation));
            }
        }
        frames.push(item);
    }

    let mut out = Json::obj();
    out.set("frames", frames);
    out.set("lookahead", Json::Bool(solved.lookahead()));
    let mut unresolved = Json::arr();
    for index in solved.unresolved().iter() {
        unresolved.push(Json::from(set.frames()[*index as usize].name));
    }
    out.set("unresolved", unresolved);
    out.set("collision_pairs", Json::Num(set.collision_pairs() as f64));
    out
}

fn numbers(values: &[f64]) -> Json {
    Json::Arr(values.iter().copied().map(Json::Num).collect())
}

/// The operator's workspace configuration, owned.
///
/// Same arrangement as [`FlatFrames`]: this holds the strings and the volume lists, and
/// [`Self::layers`] builds the borrowed view the kernel reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceOwned {
    layers: Vec<(String, Vec<OwnedVolume>)>,
}

#[derive(Debug, Clone, PartialEq)]
struct OwnedVolume {
    frame: String,
    shape: Shape,
}

impl WorkspaceOwned {
    /// The borrowed view, as two levels: the volumes first, then the layers over them.
    ///
    /// Two calls rather than one because `WorkspaceLayer` borrows a slice of `Volume`, and that
    /// slice has to live somewhere the caller owns for the length of the lease.
    pub fn volumes(&self) -> Vec<Vec<Volume<'_>>> {
        self.layers
            .iter()
            .map(|(_, volumes)| {
                volumes
                    .iter()
                    .map(|volume| Volume {
                        frame: volume.frame.as_str(),
                        shape: volume.shape,
                    })
                    .collect()
            })
            .collect()
    }

    /// The layers, over a view produced by [`Self::volumes`].
    pub fn layers<'a>(&'a self, volumes: &'a [Vec<Volume<'a>>]) -> Vec<WorkspaceLayer<'a>> {
        self.layers
            .iter()
            .zip(volumes.iter())
            .map(|((name, _), volumes)| WorkspaceLayer {
                name: name.as_str(),
                volumes: volumes.as_slice(),
            })
            .collect()
    }

    /// Whether any layer declares any volume.
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|(_, volumes)| volumes.is_empty())
    }
}

/// Read a workspace configuration (§38.2's layers, §7.1's fourth guard).
///
/// A volume whose shape cannot be read is **dropped**, and that is the safe direction here and only
/// here: within a layer the volumes are alternatives, so dropping one can only narrow what is
/// permitted. Dropping a *layer* would widen, so a layer with no readable volume is kept as an
/// empty layer — which forbids nothing about frames it does not name and, for frames it does name,
/// cannot be satisfied.
pub fn workspace_from_json(value: Option<&Json>) -> WorkspaceOwned {
    let mut out = WorkspaceOwned::default();
    let Some(value) = value else {
        return out;
    };
    for layer in value.as_arr().unwrap_or(&[]) {
        let name = layer
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let mut volumes: Vec<OwnedVolume> = Vec::new();
        for item in layer.arr_or_empty("volumes") {
            let Some(frame) = item.get("frame").and_then(Json::as_str) else {
                continue;
            };
            let Some(shape) = item.get_present("shape").and_then(shape_from_json) else {
                continue;
            };
            volumes.push(OwnedVolume {
                frame: frame.to_string(),
                shape,
            });
        }
        out.layers.push((name, volumes));
    }
    out
}

fn shape_from_json(value: &Json) -> Option<Shape> {
    match value.get("kind").and_then(Json::as_str)? {
        "box" => Some(Shape::Box {
            min: triple(value.get("min")?)?,
            max: triple(value.get("max")?)?,
        }),
        "sphere" => Some(Shape::Sphere {
            centre: triple(value.get("centre")?)?,
            radius_m: value.get("radius_m")?.as_f64()?,
        }),
        _ => None,
    }
}

fn triple(value: &Json) -> Option<[f64; 3]> {
    let items = value.as_arr()?;
    if items.len() != 3 {
        return None;
    }
    Some([items[0].as_f64()?, items[1].as_f64()?, items[2].as_f64()?])
}
