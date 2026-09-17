//! The cell descriptor (§55.2) — the physical counterpart of an environment.
//!
//! The Rust edition of `reference/typescript/cell.ts`. Read that file for the design; this one
//! records the port.
//!
//! §55.1 makes an environment a simulation artifact only a simulator driver can load. §55.2 names
//! its real-world analogue — where the fixtures are, which volumes are forbidden, where the safe
//! parking pose is, what the operator has declared about the space — and then refuses to make it a
//! repo kind, *"because an environment is published, versioned and shared; a cell is site-specific,
//! changes when somebody moves a table, and is nobody's intellectual property"*.
//!
//! So there is no publishing, no version and no digest here. A cell is a profile document (§38),
//! and it does two jobs: it supplies `env.*` to the precondition context (§54.2), and it supplies
//! allowed volumes to the `workspace` guard as one layer of §38.2's stack.
//!
//! # The check that only exists because §54.2 does
//!
//! `env.*` is addressed by a **dot path**, so a fact whose key contains a dot is a fact no
//! precondition can ever name: `{"door.closed": true}` and `{"door": {"closed": true}}` are the
//! same path and different documents. Nothing could have caught that before the predicate language
//! existed for it to be wrong in.
//!
//! # The third job, and it is a refusal (§87.1)
//!
//! §43 left *"multiple robots on one compute"* open and §87.1 answers it: **one kernel, one driver
//! and one black box per robot.** What that costs is paid here — two kernels cannot see each other's
//! envelope, so a constraint over the *pair* is enforced by neither — and the platform's answer is
//! to refuse the claim it cannot enforce rather than to invent a cell envelope nobody evaluates.
//!
//! # One thing the port had to decide
//!
//! Fact keys are reported **in sorted order**, in both editions. A `Json::Obj` is a `BTreeMap` and
//! has no other order to offer; the reference sorts to match, rather than reporting in the order
//! somebody typed. That is the honest answer as well as the compatible one — a canonical JSON
//! document's key order *is* sorted, so "document order" and "sorted" are the same thing by the
//! time either edition sees one.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;
use skynet_kernel::kinematics::{Shape, Volume, WorkspaceLayer};

use crate::js::js_trim;

/// A volume, owned.
///
/// `shape` is an [`Option`] because "declared and unreadable" is a fault to report rather than a
/// volume to drop: a workspace layer quietly missing the volume an operator drew is a robot with
/// more room than anybody agreed to.
#[derive(Debug, Clone, PartialEq)]
pub struct CellVolume {
    /// The frame this volume constrains.
    pub frame: String,
    /// Where it may be, or `None` when the geometry could not be read.
    pub shape: Option<Shape>,
}

/// A thing in the cell that is not the robot.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    /// What a precondition names it by.
    pub name: String,
    /// Where it is, when the operator has drawn it. Fixtures may be named without being measured.
    pub volume: Option<CellVolume>,
}

/// One joint of the pose the robot parks in.
#[derive(Debug, Clone, PartialEq)]
pub struct ParkingChannel {
    /// The command channel, in the body's own vocabulary.
    pub channel: String,
    /// Where that channel goes.
    pub position: f64,
}

/// One robot in a cell that holds more than one (§87.1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CellRobot {
    /// The robot, as the Hub and the descriptor name it. A precondition reads it as `env.robot`.
    pub slug: String,
    /// Where this robot's root frame sits in the cell's own frame, metres, or `None`.
    ///
    /// §44.2 puts the safety frame set at the robot's root, so a volume in [`CellRobot::allowed`] is
    /// in *that* robot's coordinates. Two robots' volumes therefore cannot be compared at all until
    /// somebody says where the robots are, which is §5.2 one level up: a disjointness claim with no
    /// placement is a rumour. Required from the second robot, and meaningless before it.
    pub at: Option<[f64; 3]>,
    /// Where this robot may be, in its own root frame. One layer, named for the robot.
    pub allowed: Vec<CellVolume>,
}

/// What an operator has written down about one physical space.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CellDescriptor {
    /// Site-local, and the only identity a cell has.
    pub name: String,
    /// Free facts about the space, reaching preconditions as `env.<key>`. Sorted by key.
    pub facts: Vec<(String, Json)>,
    /// Where the robot may be. One layer of §38.2's stack.
    pub allowed: Vec<CellVolume>,
    /// What else is in the cell.
    pub fixtures: Vec<Fixture>,
    /// Where the robot parks when a skill ends or is stopped (§48).
    pub parking_pose: Vec<ParkingChannel>,
    /// The robots in this cell (§87.1).
    ///
    /// **Absent, empty and one robot are the same cell** — the one §55.2 already describes — so
    /// every cell written before §87 is unchanged. The placement and disjointness rules apply only
    /// from the second robot, because with one robot there is no pair and the cell *is* its frame.
    pub robots: Vec<CellRobot>,
}

/// Facts this module derives, which a cell's own facts may not shadow.
///
/// A precondition reading `env.cell` must be reading the cell's name, on every robot, whatever
/// somebody typed into their configuration.
pub const DERIVED_FACTS: [&str; 4] = ["cell", "fixtures", "robot", "robots"];

/// What can be wrong with a cell descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellFaultCode {
    /// The cell has no name, and a name is the only identity it has.
    Unnamed,
    /// A fact key no dot path can address.
    FactKeyUnusable,
    /// A fact key that would shadow a derived one.
    FactKeyReserved,
    /// A volume that names no frame.
    VolumeFrameMissing,
    /// A volume with no usable geometry.
    VolumeShapeInvalid,
    /// A fixture with no name.
    FixtureUnnamed,
    /// Two fixtures with one name.
    DuplicateFixture,
    /// A parking entry with no channel.
    ParkingChannelUnnamed,
    /// One channel with two parking positions.
    DuplicateParkingChannel,
    /// A robot a precondition cannot name (§87.1).
    RobotUnnamed,
    /// Two robots with one slug.
    DuplicateRobot,
    /// Two or more robots and one carries no placement.
    RobotUnplaced,
    /// Two or more robots and one draws no volume.
    RobotUnbounded,
    /// Two robots' volumes, placed, share a point.
    SharedVolume,
}

impl CellFaultCode {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            CellFaultCode::Unnamed => "unnamed",
            CellFaultCode::FactKeyUnusable => "fact_key_unusable",
            CellFaultCode::FactKeyReserved => "fact_key_reserved",
            CellFaultCode::VolumeFrameMissing => "volume_frame_missing",
            CellFaultCode::VolumeShapeInvalid => "volume_shape_invalid",
            CellFaultCode::FixtureUnnamed => "fixture_unnamed",
            CellFaultCode::DuplicateFixture => "duplicate_fixture",
            CellFaultCode::ParkingChannelUnnamed => "parking_channel_unnamed",
            CellFaultCode::DuplicateParkingChannel => "duplicate_parking_channel",
            CellFaultCode::RobotUnnamed => "robot_unnamed",
            CellFaultCode::DuplicateRobot => "duplicate_robot",
            CellFaultCode::RobotUnplaced => "robot_unplaced",
            CellFaultCode::RobotUnbounded => "robot_unbounded",
            CellFaultCode::SharedVolume => "shared_volume",
        }
    }
}

/// Every fault code, in the reference's declaration order.
pub const CELL_FAULTS: [CellFaultCode; 14] = [
    CellFaultCode::Unnamed,
    CellFaultCode::FactKeyUnusable,
    CellFaultCode::FactKeyReserved,
    CellFaultCode::VolumeFrameMissing,
    CellFaultCode::VolumeShapeInvalid,
    CellFaultCode::FixtureUnnamed,
    CellFaultCode::DuplicateFixture,
    CellFaultCode::ParkingChannelUnnamed,
    CellFaultCode::DuplicateParkingChannel,
    CellFaultCode::RobotUnnamed,
    CellFaultCode::DuplicateRobot,
    CellFaultCode::RobotUnplaced,
    CellFaultCode::RobotUnbounded,
    CellFaultCode::SharedVolume,
];

/// One thing wrong with a cell descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct CellFault {
    /// What is wrong.
    pub code: CellFaultCode,
    /// What is wrong, by name — a key, a fixture, a channel.
    pub subjects: Vec<String>,
    /// What to tell the operator who wrote it.
    pub detail: String,
}

/// A key a dot path can address: non-empty, no dot, and no surrounding whitespace.
fn usable_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('.') && js_trim(key) == key
}

fn shape_is_usable(volume: &CellVolume) -> bool {
    match volume.shape {
        None => false,
        Some(Shape::Sphere { centre, radius_m }) => {
            radius_m.is_finite() && radius_m > 0.0 && centre.iter().all(|value| value.is_finite())
        }
        Some(Shape::Box { min, max }) => {
            min.iter().all(|value| value.is_finite())
                && max.iter().all(|value| value.is_finite())
                // A box whose min exceeds its max on any axis is empty, and an empty *allowed*
                // volume is a cell the robot may never be in.
                && (0..3).all(|axis| min[axis] <= max[axis])
        }
    }
}

fn frame_is_named(volume: &CellVolume) -> bool {
    !js_trim(&volume.frame).is_empty()
}

// ── §87.1's intersection test ────────────────────────────────────────────────

/// A shape moved into the cell's frame by its robot's placement.
///
/// Translation only. §44.4's transforms carry a rotation and this deliberately does not read one: a
/// rotated box is not a box, so the test would need an oriented-box routine whose disagreements
/// between eight editions would be in the last bits of a square root. A cell whose robots are
/// mounted at an angle is one whose operator draws axis-aligned volumes large enough to hold the
/// rotation, which over-reports sharing and never under-reports it.
fn placed(shape: Shape, at: [f64; 3]) -> Shape {
    match shape {
        Shape::Sphere { centre, radius_m } => Shape::Sphere {
            centre: [centre[0] + at[0], centre[1] + at[1], centre[2] + at[2]],
            radius_m,
        },
        Shape::Box { min, max } => Shape::Box {
            min: [min[0] + at[0], min[1] + at[1], min[2] + at[2]],
            max: [max[0] + at[0], max[1] + at[1], max[2] + at[2]],
        },
    }
}

/// Squared distance from a point to a box, zero inside.
fn box_point_distance_squared(min: [f64; 3], max: [f64; 3], point: [f64; 3]) -> f64 {
    let mut total = 0.0;
    for axis in 0..3 {
        let value = point[axis];
        let outside = if value < min[axis] {
            min[axis] - value
        } else if value > max[axis] {
            value - max[axis]
        } else {
            0.0
        };
        total += outside * outside;
    }
    total
}

/// Whether two placed shapes share a point.
///
/// **Inclusive, and that is the decision rather than an off-by-one.** `margin` counts zero as
/// inside — a workspace trip is `margin < 0` — so two robots whose boxes meet on a face can each
/// legally be at the point where they meet. A strict test would call that configuration disjoint,
/// and the platform would have certified the one arrangement where both arms are allowed into the
/// same millimetre.
fn intersects(a: Shape, b: Shape) -> bool {
    match (a, b) {
        (
            Shape::Box {
                min: amin,
                max: amax,
            },
            Shape::Box {
                min: bmin,
                max: bmax,
            },
        ) => (0..3).all(|axis| amin[axis] <= bmax[axis] && bmin[axis] <= amax[axis]),
        (
            Shape::Sphere {
                centre: ac,
                radius_m: ar,
            },
            Shape::Sphere {
                centre: bc,
                radius_m: br,
            },
        ) => {
            let dx = ac[0] - bc[0];
            let dy = ac[1] - bc[1];
            let dz = ac[2] - bc[2];
            let reach = ar + br;
            dx * dx + dy * dy + dz * dz <= reach * reach
        }
        (Shape::Box { min, max }, Shape::Sphere { centre, radius_m })
        | (Shape::Sphere { centre, radius_m }, Shape::Box { min, max }) => {
            box_point_distance_squared(min, max, centre) <= radius_m * radius_m
        }
    }
}

/// The placement, or `None` when the operator wrote none the arithmetic can use.
fn read_placement(robot: &CellRobot) -> Option<[f64; 3]> {
    let at = robot.at?;
    if at.iter().all(|value| value.is_finite()) {
        Some(at)
    } else {
        None
    }
}

/// A robot's usable volumes, placed. A volume with no usable shape is not compared.
fn placed_volumes(robot: &CellRobot) -> Vec<(String, Shape)> {
    let Some(at) = read_placement(robot) else {
        return Vec::new();
    };
    robot
        .allowed
        .iter()
        .filter(|volume| shape_is_usable(volume))
        .filter_map(|volume| {
            volume
                .shape
                .map(|shape| (volume.frame.clone(), placed(shape, at)))
        })
        .collect()
}

/// §87.1's five faults, or none.
///
/// Split out rather than inlined because all five are conditional on the same thing — **two or more
/// robots** — and a reader has to be able to see that condition once. One robot is §55.2's cell and
/// has no pair to be disjoint from; the placement rule would otherwise refuse every cell anybody
/// has ever written.
///
/// Reported in declaration order, then the pairs in index order, so every edition names the same
/// fault first. The disjointness walk skips a robot that already has a fault of its own, for
/// [`validate_cell`]'s duplicate-fixture reason: an operator told that two robots share a volume,
/// when one of them is the unplaced robot they have just been told about, would be reading one
/// problem twice.
fn robot_faults(cell: &CellDescriptor) -> Vec<CellFault> {
    let mut faults = Vec::new();
    let mut slugs: Vec<&str> = Vec::new();
    let mut comparable: Vec<(String, Vec<(String, Shape)>)> = Vec::new();
    let several = cell.robots.len() >= 2;

    for (index, robot) in cell.robots.iter().enumerate() {
        if js_trim(&robot.slug).is_empty() {
            faults.push(CellFault {
                code: CellFaultCode::RobotUnnamed,
                subjects: alloc::vec![format!("robots[{index}]")],
                detail: "A robot a precondition cannot name is a robot no skill can ask about. \
                         `env.robot` is its slug."
                    .to_string(),
            });
            continue;
        }
        if slugs.contains(&robot.slug.as_str()) {
            faults.push(CellFault {
                code: CellFaultCode::DuplicateRobot,
                subjects: alloc::vec![robot.slug.clone()],
                detail: "Two entries with one slug: a precondition asking about it would be \
                         asking about whichever the reader happened to reach."
                    .to_string(),
            });
            continue;
        }
        slugs.push(robot.slug.as_str());

        // Everything below is about the pair, so a cell with one robot stops here.
        if !several {
            continue;
        }

        if read_placement(robot).is_none() {
            faults.push(CellFault {
                code: CellFaultCode::RobotUnplaced,
                subjects: alloc::vec![robot.slug.clone()],
                detail: "This cell holds more than one robot, and this one carries no `at`. Its \
                         volumes are in its own root frame (§44.2), so nothing can say whether \
                         they overlap another robot's until somebody says where it stands."
                    .to_string(),
            });
            continue;
        }
        let volumes = placed_volumes(robot);
        if volumes.is_empty() {
            faults.push(CellFault {
                code: CellFaultCode::RobotUnbounded,
                subjects: alloc::vec![robot.slug.clone()],
                detail: "This cell holds more than one robot, and this one draws no volume the \
                         workspace guard can evaluate. A robot that may be anywhere is not \
                         disjoint from anything."
                    .to_string(),
            });
            continue;
        }
        comparable.push((robot.slug.clone(), volumes));
    }

    for a in 0..comparable.len() {
        for b in (a + 1)..comparable.len() {
            let (first_slug, first_volumes) = &comparable[a];
            let (second_slug, second_volumes) = &comparable[b];
            for (one_frame, one_shape) in first_volumes {
                for (other_frame, other_shape) in second_volumes {
                    if !intersects(*one_shape, *other_shape) {
                        continue;
                    }
                    faults.push(CellFault {
                        code: CellFaultCode::SharedVolume,
                        subjects: alloc::vec![
                            first_slug.clone(),
                            second_slug.clone(),
                            one_frame.clone(),
                            other_frame.clone()
                        ],
                        detail: format!(
                            "{first_slug}'s {one_frame} and {second_slug}'s {other_frame} share a \
                             point once both robots are placed. §87.1 gives each robot its own \
                             kernel, so nothing on this machine evaluates the pair — the overlap \
                             is a claim the platform cannot enforce rather than a hazard it has \
                             measured."
                        ),
                    });
                }
            }
        }
    }

    faults
}

/// Every fault in a cell descriptor.
///
/// A **list** rather than the first one, unlike `validate_frames` next door, and the difference is
/// about who is reading. A frame graph that does not form a tree cannot be analysed further. A cell
/// descriptor's problems are independent, an operator is editing it by hand, and `admit_package`'s
/// reason applies unchanged: fixing four problems one deployment at a time is how people end up
/// giving up and granting everything.
pub fn validate_cell(cell: &CellDescriptor) -> Vec<CellFault> {
    let mut faults = Vec::new();

    if js_trim(&cell.name).is_empty() {
        faults.push(CellFault {
            code: CellFaultCode::Unnamed,
            subjects: Vec::new(),
            detail: "A cell has no identity but its name, and this one has none.".to_string(),
        });
    }

    for (key, _) in &cell.facts {
        if !usable_key(key) {
            faults.push(CellFault {
                code: CellFaultCode::FactKeyUnusable,
                subjects: alloc::vec![key.clone()],
                detail: format!(
                    "A precondition addresses this as env.{key}, and a dot path cannot name it. \
                     Nest the object instead of putting a dot in the key."
                ),
            });
            continue;
        }
        if DERIVED_FACTS.contains(&key.as_str()) {
            faults.push(CellFault {
                code: CellFaultCode::FactKeyReserved,
                subjects: alloc::vec![key.clone()],
                detail: format!(
                    "env.{key} is derived from the cell itself, so a fact of that name would be \
                     read by nobody and would make a precondition true for the wrong reason."
                ),
            });
        }
    }

    for (index, volume) in cell.allowed.iter().enumerate() {
        if !frame_is_named(volume) {
            faults.push(CellFault {
                code: CellFaultCode::VolumeFrameMissing,
                subjects: alloc::vec![format!("allowed[{index}]")],
                detail:
                    "A volume constrains a frame, and this one names none. The workspace guard \
                         would have nothing to check it against."
                        .to_string(),
            });
        }
        if !shape_is_usable(volume) {
            faults.push(CellFault {
                code: CellFaultCode::VolumeShapeInvalid,
                subjects: alloc::vec![format!("allowed[{index}]")],
                detail: "This volume has no usable geometry — a box whose minimum exceeds its \
                         maximum, a sphere with no radius, or a shape this edition does not know."
                    .to_string(),
            });
        }
    }

    let mut seen_fixtures: Vec<&str> = Vec::new();
    for (index, fixture) in cell.fixtures.iter().enumerate() {
        if js_trim(&fixture.name).is_empty() {
            faults.push(CellFault {
                code: CellFaultCode::FixtureUnnamed,
                subjects: alloc::vec![format!("fixtures[{index}]")],
                detail:
                    "A fixture a precondition cannot name is a fixture the OS cannot check for."
                        .to_string(),
            });
            continue;
        }
        if seen_fixtures.contains(&fixture.name.as_str()) {
            faults.push(CellFault {
                code: CellFaultCode::DuplicateFixture,
                subjects: alloc::vec![fixture.name.clone()],
                detail: "Two fixtures with one name: a precondition asking for it would be asking \
                         about whichever the reader happened to reach."
                    .to_string(),
            });
            continue;
        }
        seen_fixtures.push(fixture.name.as_str());
        if let Some(volume) = &fixture.volume {
            if !shape_is_usable(volume) {
                faults.push(CellFault {
                    code: CellFaultCode::VolumeShapeInvalid,
                    subjects: alloc::vec![fixture.name.clone()],
                    detail: "This fixture has been given a volume with no usable geometry. A \
                             fixture may be named without being measured; it may not be measured \
                             wrongly."
                        .to_string(),
                });
            }
        }
    }

    let mut seen_channels: Vec<&str> = Vec::new();
    for (index, entry) in cell.parking_pose.iter().enumerate() {
        if js_trim(&entry.channel).is_empty() {
            faults.push(CellFault {
                code: CellFaultCode::ParkingChannelUnnamed,
                subjects: alloc::vec![format!("parking_pose[{index}]")],
                detail: "A parking pose is a command, and a command names its channel.".to_string(),
            });
            continue;
        }
        if seen_channels.contains(&entry.channel.as_str()) {
            faults.push(CellFault {
                code: CellFaultCode::DuplicateParkingChannel,
                subjects: alloc::vec![entry.channel.clone()],
                detail: "One channel, two parking positions. Which one the robot drives to would \
                         depend on the order a reader happened to walk the list."
                    .to_string(),
            });
            continue;
        }
        seen_channels.push(entry.channel.as_str());
    }

    faults.extend(robot_faults(cell));

    faults
}

/// The `env.*` branch of a precondition context (§54.2).
///
/// Two derived facts and then the operator's own. Derived facts are unshadowable, which
/// [`validate_cell`] enforces rather than this function silently arbitrating.
///
/// A cell with faults still produces a context. Refusing to would mean an operator's typo in a
/// fixture name took away the battery check as well, and §54.2 already has an answer for a fact
/// that cannot be read: the precondition naming it is refused, individually, with the path named.
pub fn cell_environment(cell: &CellDescriptor, subject: Option<&str>) -> Json {
    let mut environment = Json::obj();
    for (key, value) in &cell.facts {
        if usable_key(key) && !DERIVED_FACTS.contains(&key.as_str()) {
            environment.set(key, value.clone());
        }
    }
    environment.set("cell", Json::Str(cell.name.clone()));
    environment.set(
        "fixtures",
        Json::Arr(
            cell.fixtures
                .iter()
                .filter(|fixture| !js_trim(&fixture.name).is_empty())
                .map(|fixture| Json::Str(fixture.name.clone()))
                .collect(),
        ),
    );
    // §87.1. `env.robot` answers *which robot does this cell say I am*, so a slug the cell has not
    // been told about is reported as the empty string rather than echoed back: an edition that
    // echoed it would let a precondition assert membership of a cell by asserting it.
    let declared = cell_robot_slugs(cell);
    let mine = match subject {
        Some(slug) if declared.iter().any(|known| known == slug) => slug.to_string(),
        _ => String::new(),
    };
    environment.set("robot", Json::Str(mine));
    environment.set(
        "robots",
        Json::Arr(declared.into_iter().map(Json::Str).collect()),
    );
    environment
}

/// Every slug the cell names, sorted and without the ones it got wrong (§87.1).
///
/// Sorted for [`validate_cell`]'s reason one field along: a precondition asks `includes`, so order
/// is not semantic — but two editions emitting one list in two orders is a corpus failure, and
/// sorted is the order every edition can produce. A duplicate appears once, because `env.robots`
/// answers *which robots*, and `duplicate_robot` is where the document's mistake is reported.
pub fn cell_robot_slugs(cell: &CellDescriptor) -> Vec<String> {
    let mut slugs: Vec<String> = Vec::new();
    for robot in &cell.robots {
        if js_trim(&robot.slug).is_empty() || slugs.iter().any(|known| known == &robot.slug) {
            continue;
        }
        slugs.push(robot.slug.clone());
    }
    slugs.sort();
    slugs
}

/// The cell's contribution to the `workspace` guard, owned.
///
/// [`CellLayer::volumes`] and [`CellLayer::layer`] build the borrowed view the kernel reads, the
/// same two-call arrangement `WorkspaceOwned` uses and for the same reason: a `WorkspaceLayer`
/// borrows a slice of `Volume`, and that slice has to live somewhere the caller owns.
#[derive(Debug, Clone, PartialEq)]
pub struct CellLayer {
    /// `cell:<name>`, so an operator reading a workspace trip can tell which layer stopped it.
    pub name: String,
    /// The volumes, all of them usable — see [`cell_workspace_layer`].
    pub volumes: Vec<CellVolume>,
}

impl CellLayer {
    /// The volumes, borrowed.
    pub fn volumes(&self) -> Vec<Volume<'_>> {
        self.volumes
            .iter()
            .filter_map(|volume| {
                volume.shape.map(|shape| Volume {
                    frame: volume.frame.as_str(),
                    shape,
                })
            })
            .collect()
    }

    /// The layer, over a view produced by [`Self::volumes`].
    pub fn layer<'a>(&'a self, volumes: &'a [Volume<'a>]) -> WorkspaceLayer<'a> {
        WorkspaceLayer {
            name: self.name.as_str(),
            volumes,
        }
    }
}

/// The cell's workspace layer, or `None` when it draws no usable volumes.
///
/// Layers compose by conjunction — a frame must be inside at least one volume of *every* layer that
/// names it — so adding this one can only narrow what the site above it allowed. §38.2's rule holds
/// by construction rather than by a check.
///
/// **A cell that draws nothing adds no layer**, and that is not the same as a cell that allows
/// nothing. An empty layer would forbid every frame it named, and a site that has not yet measured
/// its cell would find the robot refusing to move with no volume anywhere to explain why.
pub fn cell_workspace_layer(cell: &CellDescriptor) -> Option<CellLayer> {
    let volumes: Vec<CellVolume> = cell
        .allowed
        .iter()
        .filter(|volume| shape_is_usable(volume) && frame_is_named(volume))
        .cloned()
        .collect();
    if volumes.is_empty() {
        return None;
    }
    Some(CellLayer {
        name: format!("cell:{}", cell.name),
        volumes,
    })
}

/// The layer one robot of a shared cell contributes, or `None` (§87.1).
///
/// A **second** layer rather than a replacement for [`cell_workspace_layer`], because §38.2's
/// layers compose by conjunction and two layers is how a cell-wide volume and a per-robot one both
/// hold. A caller that substituted this for the cell's own would be letting a robot's own volume
/// *widen* what the cell allowed everything, which is the one thing a layer may not do.
///
/// The volumes are **unplaced**, in the robot's own root frame, because that is the frame the
/// `workspace` guard resolves poses in (§44.2). `at` exists for the disjointness test in
/// [`validate_cell`] and for nothing else; a guard given cell-frame volumes would be comparing two
/// coordinate systems and reporting the difference as a margin.
pub fn cell_robot_layer(cell: &CellDescriptor, slug: &str) -> Option<CellLayer> {
    if js_trim(slug).is_empty() {
        return None;
    }
    let robot = cell.robots.iter().find(|robot| robot.slug == slug)?;
    let volumes: Vec<CellVolume> = robot
        .allowed
        .iter()
        .filter(|volume| shape_is_usable(volume) && frame_is_named(volume))
        .cloned()
        .collect();
    if volumes.is_empty() {
        return None;
    }
    Some(CellLayer {
        name: format!("cell:{}:{}", cell.name, slug),
        volumes,
    })
}

// ── Reading and writing ──────────────────────────────────────────────────────

fn triple(value: Option<&Json>) -> Option<[f64; 3]> {
    let items = value?.as_arr()?;
    if items.len() != 3 {
        return None;
    }
    Some([items[0].as_f64()?, items[1].as_f64()?, items[2].as_f64()?])
}

fn shape_from_json(value: Option<&Json>) -> Option<Shape> {
    let value = value?;
    match value.get("kind").and_then(Json::as_str)? {
        "box" => Some(Shape::Box {
            min: triple(value.get("min"))?,
            max: triple(value.get("max"))?,
        }),
        "sphere" => Some(Shape::Sphere {
            centre: triple(value.get("centre"))?,
            radius_m: value.get("radius_m")?.as_f64()?,
        }),
        _ => None,
    }
}

fn volume_from_json(value: &Json) -> CellVolume {
    CellVolume {
        frame: value
            .get("frame")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        shape: shape_from_json(value.get_present("shape")),
    }
}

/// Read a cell descriptor.
///
/// Lenient by design: everything wrong with a document is a fault [`validate_cell`] reports, not a
/// decode error that stops an operator seeing the other four. A missing field reads as absent and a
/// malformed one reads as unusable, and both are named rather than dropped.
pub fn cell_from_json(value: &Json) -> CellDescriptor {
    let facts = match value.get_present("facts").and_then(Json::as_obj) {
        // A `BTreeMap` iterates sorted, which is the order the reference sorts to match. A
        // canonical JSON document's key order *is* sorted, so this is document order as well.
        Some(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        None => Vec::new(),
    };

    CellDescriptor {
        name: value
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        facts,
        allowed: value
            .arr_or_empty("allowed")
            .iter()
            .map(volume_from_json)
            .collect(),
        fixtures: value
            .arr_or_empty("fixtures")
            .iter()
            .map(|entry| Fixture {
                name: entry
                    .get("name")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
                volume: entry.get_present("volume").map(volume_from_json),
            })
            .collect(),
        parking_pose: value
            .arr_or_empty("parking_pose")
            .iter()
            .map(|entry| ParkingChannel {
                channel: entry
                    .get("channel")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
                position: entry.get("position").and_then(Json::as_f64).unwrap_or(0.0),
            })
            .collect(),
        robots: value
            .arr_or_empty("robots")
            .iter()
            .map(|entry| CellRobot {
                slug: entry
                    .get("slug")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
                // `triple` refuses anything that is not three finite-shaped numbers, so a
                // two-element placement and a placement of strings both read as absent — which is
                // `robot_unplaced`, not a placement with a zero in it.
                at: triple(entry.get_present("at")),
                allowed: entry
                    .arr_or_empty("allowed")
                    .iter()
                    .map(volume_from_json)
                    .collect(),
            })
            .collect(),
    }
}

fn shape_to_json(shape: &Shape) -> Json {
    let mut out = Json::obj();
    match shape {
        Shape::Box { min, max } => {
            out.set("kind", Json::Str("box".to_string()))
                .set(
                    "min",
                    Json::Arr(min.iter().map(|v| Json::Num(*v)).collect()),
                )
                .set(
                    "max",
                    Json::Arr(max.iter().map(|v| Json::Num(*v)).collect()),
                );
        }
        Shape::Sphere { centre, radius_m } => {
            out.set("kind", Json::Str("sphere".to_string()))
                .set(
                    "centre",
                    Json::Arr(centre.iter().map(|v| Json::Num(*v)).collect()),
                )
                .set("radius_m", Json::Num(*radius_m));
        }
    }
    out
}

/// The wire shape of [`validate_cell`]'s answer.
pub fn cell_faults_to_json(faults: &[CellFault]) -> Json {
    let mut out = Json::obj();
    out.set(
        "faults",
        Json::Arr(
            faults
                .iter()
                .map(|fault| {
                    let mut body = Json::obj();
                    body.set("code", Json::Str(fault.code.as_str().to_string()))
                        .set(
                            "subjects",
                            Json::Arr(fault.subjects.iter().cloned().map(Json::Str).collect()),
                        )
                        .set("detail", Json::Str(fault.detail.clone()));
                    body
                })
                .collect(),
        ),
    );
    out
}

/// The wire shape of [`cell_workspace_layer`], `{"layer":null}` when the cell draws nothing.
pub fn cell_layer_to_json(layer: &Option<CellLayer>) -> Json {
    let mut out = Json::obj();
    match layer {
        None => {
            out.set("layer", Json::Null);
        }
        Some(layer) => {
            let mut body = Json::obj();
            body.set("name", Json::Str(layer.name.clone())).set(
                "volumes",
                Json::Arr(
                    layer
                        .volumes
                        .iter()
                        .map(|volume| {
                            let mut entry = Json::obj();
                            entry.set("frame", Json::Str(volume.frame.clone())).set(
                                "shape",
                                volume.shape.as_ref().map_or(Json::Null, shape_to_json),
                            );
                            entry
                        })
                        .collect(),
                ),
            );
            out.set("layer", body);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn read(source: &str) -> CellDescriptor {
        cell_from_json(&Json::parse(source).expect("the cell parses"))
    }

    fn codes(cell: &CellDescriptor) -> Vec<&'static str> {
        validate_cell(cell)
            .iter()
            .map(|fault| fault.code.as_str())
            .collect()
    }

    /// The reader is lenient on purpose: everything wrong with a document is a fault an operator
    /// can read, not a decode error that stops them seeing the other four.
    #[test]
    fn a_document_missing_everything_still_reads() {
        let cell = read("{}");
        assert_eq!(cell.name, "");
        assert!(cell.facts.is_empty());
        assert!(cell.allowed.is_empty());
        assert!(cell.fixtures.is_empty());
        assert!(cell.parking_pose.is_empty());
        assert_eq!(codes(&cell), ["unnamed"]);
    }

    #[test]
    fn fields_of_the_wrong_type_read_as_absent_rather_than_failing() {
        let cell =
            read(r#"{"name":5,"facts":"nope","allowed":7,"fixtures":null,"parking_pose":{}}"#);
        assert_eq!(cell.name, "");
        assert!(cell.facts.is_empty());
        assert!(cell.allowed.is_empty());
    }

    /// The check that only exists because §54.2 gave `env.*` a path vocabulary.
    #[test]
    fn a_fact_key_a_path_cannot_name_is_refused() {
        assert_eq!(
            codes(&read(r#"{"name":"bay-3","facts":{"door.closed":true}}"#)),
            ["fact_key_unusable"]
        );
        assert_eq!(
            codes(&read(r#"{"name":"bay-3","facts":{" padded ":true}}"#)),
            ["fact_key_unusable"]
        );
        assert_eq!(
            codes(&read(r#"{"name":"bay-3","facts":{"":true}}"#)),
            ["fact_key_unusable"]
        );
        // And a key that would shadow a derived fact, which is a different fault with a different
        // remedy: the key is addressable, it is just already taken.
        assert_eq!(
            codes(&read(r#"{"name":"bay-3","facts":{"cell":"elsewhere"}}"#)),
            ["fact_key_reserved"]
        );
    }

    #[test]
    fn an_unusable_fact_never_reaches_the_context() {
        let cell = read(r#"{"name":"bay-3","facts":{"door.closed":true,"door_open":false}}"#);
        let env = cell_environment(&cell, None);
        assert!(env.get("door.closed").is_none());
        assert_eq!(env.get("door_open").and_then(Json::as_bool), Some(false));
        // The derived facts are always there, and they win.
        assert_eq!(env.get("cell").and_then(Json::as_str), Some("bay-3"));
        assert_eq!(
            env.get("fixtures")
                .and_then(Json::as_arr)
                .map(<[Json]>::len),
            Some(0)
        );
    }

    #[test]
    fn a_shadowing_fact_does_not_win() {
        let env = cell_environment(
            &read(r#"{"name":"bay-3","facts":{"cell":"elsewhere"}}"#),
            None,
        );
        assert_eq!(env.get("cell").and_then(Json::as_str), Some("bay-3"));
    }

    /// JSON cannot hold a non-finite number, so this is the only place the check can be exercised.
    #[test]
    fn a_volume_with_non_finite_geometry_is_unusable() {
        let cell = CellDescriptor {
            name: "bay-3".to_string(),
            allowed: vec![CellVolume {
                frame: "tool0".to_string(),
                shape: Some(Shape::Sphere {
                    centre: [0.0, 0.0, f64::NAN],
                    radius_m: 1.0,
                }),
            }],
            ..CellDescriptor::default()
        };
        assert_eq!(codes(&cell), ["volume_shape_invalid"]);
        assert!(cell_workspace_layer(&cell).is_none());

        let infinite = CellDescriptor {
            name: "bay-3".to_string(),
            allowed: vec![CellVolume {
                frame: "tool0".to_string(),
                shape: Some(Shape::Box {
                    min: [0.0, 0.0, 0.0],
                    max: [f64::INFINITY, 1.0, 1.0],
                }),
            }],
            ..CellDescriptor::default()
        };
        assert_eq!(codes(&infinite), ["volume_shape_invalid"]);
    }

    /// A cell that draws nothing adds no layer, which is not the same as a cell that allows
    /// nothing: an empty layer would forbid every frame it named.
    #[test]
    fn no_volumes_means_no_layer() {
        assert!(cell_workspace_layer(&read(r#"{"name":"bay-4"}"#)).is_none());
    }

    /// The borrowed view is the point of the owned type, so it is worth proving it reaches the
    /// kernel's shape rather than only that it compiles.
    #[test]
    fn the_layer_reaches_the_kernel_as_a_workspace_layer() {
        let cell = read(
            r#"{"name":"bay-3","allowed":[
                 {"frame":"tool0","shape":{"kind":"box","min":[-1,-1,0],"max":[1,1,2]}},
                 {"frame":"base","shape":{"kind":"cylinder"}}]}"#,
        );
        // The unreadable one is reported and is not handed to the guard.
        assert_eq!(codes(&cell), ["volume_shape_invalid"]);

        let owned = cell_workspace_layer(&cell).expect("one usable volume");
        assert_eq!(owned.name, "cell:bay-3");
        let volumes = owned.volumes();
        let layer: WorkspaceLayer<'_> = owned.layer(&volumes);
        assert_eq!(layer.name, "cell:bay-3");
        assert_eq!(layer.volumes.len(), 1);
        assert_eq!(layer.volumes[0].frame, "tool0");
        // And the geometry survived the round trip, which is what the guard will measure against.
        assert!(layer.volumes[0].shape.margin(&[0.0, 0.0, 1.0]) > 0.0);
        assert!(layer.volumes[0].shape.margin(&[5.0, 0.0, 1.0]) < 0.0);
    }

    #[test]
    fn duplicates_are_reported_once_each() {
        let cell = read(
            r#"{"name":"bay-3",
                "fixtures":[{"name":"jig"},{"name":"jig"}],
                "parking_pose":[{"channel":"J1","position":0},{"channel":"J1","position":1}]}"#,
        );
        assert_eq!(
            codes(&cell),
            ["duplicate_fixture", "duplicate_parking_channel"]
        );
    }

    #[test]
    fn every_fault_is_reported_not_only_the_first() {
        let cell = read(
            r#"{"name":"","facts":{"z.bad":1,"a.bad":2},
                "allowed":[{"frame":"","shape":{"kind":"box","min":[1,1,1],"max":[0,0,0]}}]}"#,
        );
        assert_eq!(
            codes(&cell),
            [
                "unnamed",
                // Sorted, which is the only order a parsed object has in every edition.
                "fact_key_unusable",
                "fact_key_unusable",
                "volume_frame_missing",
                "volume_shape_invalid",
            ]
        );
        let faults = validate_cell(&cell);
        assert_eq!(faults[1].subjects, ["a.bad"]);
        assert_eq!(faults[2].subjects, ["z.bad"]);
    }
}
