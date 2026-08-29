use super::*;

use core::f64::consts::PI;

fn close(a: f64, b: f64) -> bool {
    abs(a - b) <= 1e-12
}

fn assert_point(actual: &[f64; 3], expected: [f64; 3]) {
    for axis in 0..3 {
        assert!(
            close(actual[axis], expected[axis]),
            "axis {axis}: {actual:?} vs {expected:?}"
        );
    }
}

fn root(name: &str) -> Frame<'_> {
    Frame {
        name,
        parent: None,
        local: Transform::IDENTITY,
        joint: None,
        collision: None,
        ignore: &[],
    }
}

fn hinge<'a>(name: &'a str, parent: u16, offset: [f64; 3], joint: &'a str) -> Frame<'a> {
    Frame {
        name,
        parent: Some(parent),
        local: Transform {
            translation: offset,
            rotation: [0.0, 0.0, 0.0, 1.0],
        },
        joint: Some(Joint {
            name: joint,
            kind: JointKind::Revolute,
            axis: [0.0, 0.0, 1.0],
            mimic: None,
        }),
        collision: None,
        ignore: &[],
    }
}

/// The command, as the kernel hands it to the solver: a lookup rather than a slice.
fn lookup<'a>(pairs: &'a [(&'a str, f64)]) -> impl Fn(&str) -> Option<f64> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(channel, _)| *channel == name)
            .map(|(_, value)| *value)
    }
}

fn bound<'a>(joint: &'a str, channel: &'a str, measured: f64) -> JointBinding<'a> {
    JointBinding {
        joint,
        channel: Some(channel),
        measured,
    }
}

// ── Transforms ───────────────────────────────────────────────────────────────

#[test]
fn composing_with_the_identity_changes_nothing() {
    let t = Transform {
        translation: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };
    assert_eq!(Transform::IDENTITY.compose(&t), t);
    assert_eq!(t.compose(&Transform::IDENTITY), t);
}

#[test]
fn a_childs_offset_is_rotated_into_the_parent_before_it_is_added() {
    // The property that makes a chain a chain. A quarter turn about z, then a metre along x,
    // arrives a metre along y — not a metre along x.
    let half = 0.5 * (PI / 2.0);
    let parent = Transform {
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, sin(half), cos(half)],
    };
    let child = Transform {
        translation: [1.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };
    let world = parent.compose(&child);
    assert_point(&world.translation, [0.0, 1.0, 0.0]);
}

#[test]
fn rotation_is_active_and_right_handed() {
    // REP-103: x forward, y left, z up, and a positive rotation about z takes x towards y.
    let half = 0.5 * (PI / 2.0);
    let q = [0.0, 0.0, sin(half), cos(half)];
    assert_point(&rotate(&q, &[1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
    assert_point(&rotate(&q, &[0.0, 1.0, 0.0]), [-1.0, 0.0, 0.0]);
    assert_point(&rotate(&q, &[0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]);
}

// ── The frame set's invariant ────────────────────────────────────────────────

#[test]
fn a_set_whose_parents_come_first_is_accepted() {
    let frames = [root("base_link"), hinge("arm", 0, [0.0, 0.0, 0.0], "j")];
    assert!(FrameSet::new(&frames).is_ok());
}

#[test]
fn a_parent_after_its_child_is_refused_rather_than_sorted() {
    // The invariant is what makes forward kinematics a single pass with no cycle check inside it,
    // so it is enforced at the boundary and never again.
    let mut child = hinge("arm", 1, [0.0, 0.0, 0.0], "j");
    child.parent = Some(1);
    let frames = [root("base_link"), child];
    assert_eq!(
        FrameSet::new(&frames).unwrap_err(),
        FrameSetFault::NotTopological
    );
}

#[test]
fn a_second_root_is_refused() {
    let frames = [root("base_link"), root("other")];
    assert_eq!(
        FrameSet::new(&frames).unwrap_err(),
        FrameSetFault::RootNotFirst
    );
}

#[test]
fn an_empty_set_is_refused() {
    assert_eq!(FrameSet::new(&[]).unwrap_err(), FrameSetFault::Empty);
}

#[test]
fn a_parent_index_past_the_end_is_refused() {
    let frames = [root("base_link"), hinge("arm", 9, [0.0; 3], "j")];
    // Out of range is reported before the ordering rule, because "index 9 of a two-frame set" is
    // the more specific fact.
    assert_eq!(
        FrameSet::new(&frames).unwrap_err(),
        FrameSetFault::ParentOutOfRange
    );
}

#[test]
fn an_ignore_entry_past_the_end_is_refused() {
    let mut frame = hinge("arm", 0, [0.0; 3], "j");
    frame.ignore = &[7];
    let frames = [root("base_link"), frame];
    assert_eq!(
        FrameSet::new(&frames).unwrap_err(),
        FrameSetFault::IgnoreOutOfRange
    );
}

// ── Forward kinematics ───────────────────────────────────────────────────────

#[test]
fn a_two_link_planar_arm_lands_where_trigonometry_says() {
    // Shoulder a metre up, elbow a metre out, both about z. At 90° and 0° the tool is at
    // (0, 1, 1): the first link points along +y and the second continues along it.
    let frames = [
        root("base_link"),
        hinge("upper", 0, [0.0, 0.0, 1.0], "shoulder"),
        hinge("tool", 1, [1.0, 0.0, 0.0], "elbow"),
    ];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [
        bound("shoulder", "shoulder", 0.0),
        bound("elbow", "elbow", 0.0),
    ];
    let mut k = Kinematics::new();
    k.solve(
        &set,
        &bindings,
        lookup(&[("shoulder", PI / 2.0), ("elbow", 0.0)]),
    );

    assert_point(&k.pose(2).unwrap().translation, [0.0, 1.0, 1.0]);
    assert!(k.lookahead());
    assert!(k.unresolved().is_empty());
}

#[test]
fn a_prismatic_joint_slides_along_its_axis() {
    let mut slider = hinge("carriage", 0, [0.0, 0.0, 0.0], "rail");
    slider.joint = Some(Joint {
        name: "rail",
        kind: JointKind::Prismatic,
        axis: [0.0, 0.0, 1.0],
        mimic: None,
    });
    let frames = [root("base_link"), slider];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("rail", "rail", 0.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("rail", 0.35)]));
    assert_point(&k.pose(1).unwrap().translation, [0.0, 0.0, 0.35]);
}

#[test]
fn a_mimic_follows_its_driver_through_the_multiplier_and_offset() {
    // A parallel gripper. The second finger runs at −1, and a frame graph that could only say
    // "follows" would put it on the wrong side of the object.
    let mut left = hinge("finger_left", 0, [0.0, 0.0, 0.0], "grip");
    left.joint = Some(Joint {
        name: "grip",
        kind: JointKind::Prismatic,
        axis: [0.0, 1.0, 0.0],
        mimic: None,
    });
    let mut right = hinge("finger_right", 0, [0.0, 0.0, 0.0], "grip_mirror");
    right.joint = Some(Joint {
        name: "grip_mirror",
        kind: JointKind::Prismatic,
        axis: [0.0, 1.0, 0.0],
        mimic: Some(Mimic {
            joint: "grip",
            multiplier: -1.0,
            offset: 0.0,
        }),
    });
    let frames = [root("palm"), left, right];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [
        bound("grip", "grip", 0.0),
        JointBinding {
            joint: "grip_mirror",
            channel: None,
            measured: 0.0,
        },
    ];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("grip", 0.02)]));
    assert_point(&k.pose(1).unwrap().translation, [0.0, 0.02, 0.0]);
    assert_point(&k.pose(2).unwrap().translation, [0.0, -0.02, 0.0]);
}

#[test]
fn a_ball_joint_is_unresolved_and_so_is_everything_below_it() {
    // One scalar cannot say where a ball joint is. Assuming it has not rolled would put the whole
    // limb somewhere it is not, which is worse than reporting nothing.
    let mut shoulder = hinge("shoulder", 0, [0.0; 3], "ball");
    shoulder.joint = Some(Joint {
        name: "ball",
        kind: JointKind::Ball,
        axis: [0.0, 0.0, 1.0],
        mimic: None,
    });
    let frames = [
        root("base_link"),
        shoulder,
        hinge("tool", 1, [1.0, 0.0, 0.0], "elbow"),
    ];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("ball", "ball", 0.0), bound("elbow", "elbow", 0.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("ball", 0.0), ("elbow", 0.0)]));

    assert!(k.pose(0).is_some(), "the root still resolves");
    assert!(k.pose(1).is_none());
    assert!(k.pose(2).is_none(), "unresolved propagates down the chain");
    assert_eq!(k.unresolved().len(), 2);
}

#[test]
fn a_joint_with_no_binding_leaves_its_frame_unresolved() {
    let frames = [root("base_link"), hinge("arm", 0, [0.0; 3], "elbow")];
    let set = FrameSet::new(&frames).unwrap();
    let mut k = Kinematics::new();
    k.solve(&set, &[], lookup(&[("elbow", 1.0)]));
    assert!(k.pose(1).is_none());
}

#[test]
fn a_joint_the_command_did_not_carry_falls_back_to_the_state_and_says_so() {
    // The check still runs — where the body is now is a real thing to check — but it is no longer
    // the lookahead §7.1 promises, and the verdict has to be able to say which was done.
    let frames = [root("base_link"), hinge("arm", 0, [1.0, 0.0, 0.0], "elbow")];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("elbow", "elbow", PI / 2.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[]));
    assert!(k.pose(1).is_some());
    assert!(!k.lookahead());
}

#[test]
fn a_velocity_commanded_body_checks_where_it_is_rather_than_where_it_is_going() {
    let frames = [root("base_link"), hinge("arm", 0, [1.0, 0.0, 0.0], "elbow")];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [JointBinding {
        joint: "elbow",
        channel: None,
        measured: 0.0,
    }];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("elbow_velocity", 4.0)]));
    assert_point(&k.pose(1).unwrap().translation, [1.0, 0.0, 0.0]);
    assert!(!k.lookahead());
}

#[test]
fn a_degenerate_axis_is_a_defect_and_not_a_fixed_joint() {
    let mut frame = hinge("arm", 0, [0.0; 3], "elbow");
    frame.joint = Some(Joint {
        name: "elbow",
        kind: JointKind::Revolute,
        axis: [0.0, 0.0, 0.0],
        mimic: None,
    });
    let frames = [root("base_link"), frame];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("elbow", "elbow", 0.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("elbow", 1.0)]));
    assert!(k.pose(1).is_none());
}

#[test]
fn an_angle_past_the_specified_reductions_range_leaves_the_frame_unresolved() {
    // The arithmetic refuses; the kinematics reads that as an unresolved frame rather than letting
    // a NaN reach a comparison, where it would compare false and read as "inside the volume".
    let frames = [root("base_link"), hinge("wheel", 0, [0.0; 3], "axle")];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("axle", "axle", 0.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("axle", 5.0e6)]));
    assert!(k.pose(1).is_none());
}

#[test]
fn solving_twice_gives_the_same_answer() {
    // The scratch buffer is reused every cycle; a residue from the previous solve would be a fault
    // that only appears on the second cycle.
    let frames = [
        root("base_link"),
        hinge("upper", 0, [0.0, 0.0, 1.0], "shoulder"),
    ];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("shoulder", "shoulder", 0.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[("shoulder", 1.0)]));
    let first = *k.pose(1).unwrap();
    k.solve(&set, &bindings, lookup(&[("shoulder", 1.0)]));
    assert_eq!(first, *k.pose(1).unwrap());
}

#[test]
fn an_unresolved_frame_does_not_survive_into_the_next_cycle() {
    let frames = [root("base_link"), hinge("arm", 0, [0.0; 3], "elbow")];
    let set = FrameSet::new(&frames).unwrap();
    let bindings = [bound("elbow", "elbow", 0.0)];
    let mut k = Kinematics::new();
    k.solve(&set, &bindings, lookup(&[]));
    assert!(!k.lookahead());
    k.solve(&set, &bindings, lookup(&[("elbow", 0.5)]));
    assert!(k.lookahead(), "the flag is per cycle, not sticky");
    assert!(k.unresolved().is_empty());
}

// ── Volumes ──────────────────────────────────────────────────────────────────

#[test]
fn a_sphere_reports_a_signed_margin() {
    let shape = Shape::Sphere {
        centre: [0.0, 0.0, 0.0],
        radius_m: 1.0,
    };
    assert!(close(shape.margin(&[0.0, 0.0, 0.0]), 1.0));
    assert!(close(shape.margin(&[0.5, 0.0, 0.0]), 0.5));
    assert!(close(shape.margin(&[1.0, 0.0, 0.0]), 0.0));
    assert!(close(shape.margin(&[2.0, 0.0, 0.0]), -1.0));
}

#[test]
fn a_box_reports_a_signed_margin_inside_and_out() {
    let shape = Shape::Box {
        min: [-1.0, -1.0, -1.0],
        max: [1.0, 1.0, 1.0],
    };
    assert!(close(shape.margin(&[0.0, 0.0, 0.0]), 1.0));
    assert!(close(shape.margin(&[0.9, 0.0, 0.0]), 0.1));
    // Outside on one axis: the distance to the face.
    assert!(close(shape.margin(&[1.5, 0.0, 0.0]), -0.5));
    // Outside on two: the distance to the edge, not the sum.
    assert!(close(shape.margin(&[2.0, 2.0, 0.0]), -sqrt(2.0)));
}

// ── Capsule distance ─────────────────────────────────────────────────────────

#[test]
fn two_parallel_segments_are_their_perpendicular_distance_apart() {
    let d = segment_distance_squared(
        &[0.0, 0.0, 0.0],
        &[1.0, 0.0, 0.0],
        &[0.0, 0.5, 0.0],
        &[1.0, 0.5, 0.0],
    );
    assert!(close(distance(d), 0.5));
}

#[test]
fn two_crossing_segments_touch() {
    let d = segment_distance_squared(
        &[-1.0, 0.0, 0.0],
        &[1.0, 0.0, 0.0],
        &[0.0, -1.0, 0.0],
        &[0.0, 1.0, 0.0],
    );
    assert!(close(distance(d), 0.0));
}

#[test]
fn segments_that_pass_without_meeting_report_the_gap() {
    let d = segment_distance_squared(
        &[-1.0, 0.0, 0.0],
        &[1.0, 0.0, 0.0],
        &[0.0, -1.0, 0.25],
        &[0.0, 1.0, 0.25],
    );
    assert!(close(distance(d), 0.25));
}

#[test]
fn a_sphere_is_a_segment_of_no_length() {
    let d = segment_distance_squared(
        &[0.0, 0.0, 0.0],
        &[0.0, 0.0, 0.0],
        &[3.0, 4.0, 0.0],
        &[3.0, 4.0, 0.0],
    );
    assert!(close(distance(d), 5.0));
}

#[test]
fn the_closest_point_can_be_an_endpoint() {
    // Two collinear segments end to end with a gap: the answer is between the near endpoints and
    // not between the parameterised interiors.
    let d = segment_distance_squared(
        &[0.0, 0.0, 0.0],
        &[1.0, 0.0, 0.0],
        &[3.0, 0.0, 0.0],
        &[4.0, 0.0, 0.0],
    );
    assert!(close(distance(d), 2.0));
}

#[test]
fn a_capsule_moves_with_its_frame() {
    let half = 0.5 * (PI / 2.0);
    let pose = Transform {
        translation: [1.0, 0.0, 0.0],
        rotation: [0.0, 0.0, sin(half), cos(half)],
    };
    let capsule = Capsule {
        a: [0.0, 0.0, 0.0],
        b: [1.0, 0.0, 0.0],
        radius_m: 0.1,
    };
    let (a, b) = capsule_in_world(&pose, &capsule);
    assert_point(&a, [1.0, 0.0, 0.0]);
    assert_point(&b, [1.0, 1.0, 0.0]);
}

// ── Adjacency and the ignore list ────────────────────────────────────────────

#[test]
fn a_parent_and_its_child_are_adjacent_in_both_directions() {
    let frames = [root("base_link"), hinge("arm", 0, [0.0; 3], "j")];
    assert!(adjacent(&frames, 0, 1));
    assert!(adjacent(&frames, 1, 0));
}

#[test]
fn an_ignore_list_is_symmetric_however_it_was_written() {
    let mut torso = hinge("torso", 0, [0.0; 3], "j");
    torso.ignore = &[2];
    let frames = [root("base_link"), torso, hinge("pelvis", 0, [0.0; 3], "k")];
    assert!(ignores(&frames, 1, 2));
    assert!(ignores(&frames, 2, 1), "written once, meant both ways");
    assert!(!ignores(&frames, 0, 1));
}

#[test]
fn the_quadratic_cost_is_reported_rather_than_assumed_small() {
    let mut with_geometry = hinge("arm", 0, [0.0; 3], "j");
    with_geometry.collision = Some(Capsule {
        a: [0.0; 3],
        b: [0.0, 0.0, 1.0],
        radius_m: 0.1,
    });
    let mut second = with_geometry;
    second.name = "forearm";
    second.parent = Some(1);
    let mut third = with_geometry;
    third.name = "hand";
    third.parent = Some(2);
    let frames = [root("base_link"), with_geometry, second, third];
    let set = FrameSet::new(&frames).unwrap();
    assert_eq!(set.collision_bodies(), 3);
    assert_eq!(set.collision_pairs(), 3);
}
