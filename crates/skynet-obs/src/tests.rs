//! Tests for the observation ring.
//!
//! The cases that matter are the ones about **confusion with the command ring** and about what a
//! short observation leaves behind, because those are the two failures that would be silent on a
//! robot: a policy reading joint positions as setpoints, and a policy integrating a stale position
//! under a fresh stamp.

use super::*;
use alloc::vec::Vec;

const SLOTS: usize = 4;

fn words() -> Vec<AtomicU32> {
    (0..words_for(SLOTS)).map(|_| AtomicU32::new(0)).collect()
}

fn sample(joints: usize, stamp_ns: u64, seq: u64) -> Observation {
    let mut observation = Observation {
        stamp_ns,
        seq,
        joints,
        ..Observation::default()
    };
    for joint in 0..joints {
        observation.position[joint] = joint as f32 * 0.1;
        observation.velocity[joint] = joint as f32 * 0.01;
        observation.effort[joint] = joint as f32 * 1.5;
    }
    observation
}

/// A published observation reads back exactly.
#[test]
fn what_is_published_is_what_is_read() {
    let region = words();
    let ring = Ring::create(&region, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&region).expect("the same ring").consumer();

    let sent = sample(3, 12_345_678, 9);
    producer.publish(&sent).expect("the ring accepts it");

    let mut got = Observation::default();
    assert_eq!(consumer.read(&mut got), Reading::Fresh);
    assert_eq!(got.stamp_ns, 12_345_678);
    assert_eq!(got.seq, 9);
    assert_eq!(got.joints, 3);
    assert!((got.position[1] - 0.1).abs() < 1e-6);
    assert!((got.velocity[2] - 0.02).abs() < 1e-6);
    assert!((got.effort[2] - 3.0).abs() < 1e-6);
}

/// A ring nobody has published to is empty rather than a reading of zeros.
#[test]
fn an_unwritten_ring_is_empty() {
    let region = words();
    Ring::create(&region, SLOTS).expect("a ring");
    let mut consumer = Ring::open(&region).expect("the same ring").consumer();

    let mut got = sample(2, 5, 5);
    assert_eq!(consumer.read(&mut got), Reading::Empty);
    assert_eq!(
        got.stamp_ns, 5,
        "and the caller's buffer was not touched, so what it held is still what it holds",
    );
}

/// A command ring is not an observation ring, and the magic is where that is decided.
///
/// The failure this prevents is the reason this crate exists separately: a process that opened the
/// wrong mapping would read commanded setpoints as measured positions, or joint positions as
/// commands, under a header that validated. It fails at the first word instead.
#[test]
fn a_command_ring_is_refused_at_the_magic() {
    let region: Vec<AtomicU32> = (0..words_for(SLOTS)).map(|_| AtomicU32::new(0)).collect();
    // The command ring's magic, written by hand rather than by depending on that crate — this one
    // takes no dependency on it, and the point being tested is that the bytes differ.
    region[0].store(u32::from_le_bytes(*b"SKYR"), Ordering::Relaxed);
    region[1].store(u32::from_le_bytes(*b"ING1"), Ordering::Relaxed);
    region[2].store(1, Ordering::Relaxed);
    region[3].store(SLOT_WORDS as u32, Ordering::Relaxed);
    region[4].store(SLOTS as u32, Ordering::Relaxed);
    region[5].store(MAX_JOINTS as u32, Ordering::Relaxed);

    assert_eq!(Ring::open(&region).err(), Some(ObsError::NotARing));
}

/// A shorter observation does not leave the previous one's joints above it.
///
/// The failure is quiet and bad: a policy reads six joints, the next sample carries three, and
/// joints four to six still hold values from a sample one cycle old — under a **fresh stamp**, which
/// is exactly what makes them look current. They read as `NaN`, which is unusable rather than
/// plausible.
#[test]
fn a_shorter_observation_leaves_no_stale_joints_behind() {
    let region = words();
    let ring = Ring::create(&region, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&region).expect("the same ring").consumer();

    producer.publish(&sample(6, 1_000, 1)).expect("six joints");
    // Same slot: a ring of four slots, published four times apart.
    for seq in 2..=4 {
        producer
            .publish(&sample(6, 1_000 + seq, seq))
            .expect("more");
    }
    producer.publish(&sample(3, 9_000, 5)).expect("then three");

    let mut got = Observation::default();
    assert_eq!(consumer.read(&mut got), Reading::Fresh);
    assert_eq!(got.joints, 3);
    assert!(
        got.position[4].is_nan(),
        "a joint outside the declared count has no measurement, and NaN is what that is",
    );
    assert!(got.velocity[4].is_nan());
    assert!(got.effort[4].is_nan());
}

/// An unlocalised base is `NaN`, not the origin.
#[test]
fn a_default_observation_does_not_claim_the_robot_is_at_the_origin() {
    assert!(Observation::default().base_pose.iter().all(|v| v.is_nan()));
}

/// More joints than the ring carries is refused rather than truncated.
#[test]
fn too_many_joints_is_refused() {
    let region = words();
    let ring = Ring::create(&region, SLOTS).expect("a ring");
    let mut producer = ring.producer();

    let observation = Observation {
        joints: MAX_JOINTS + 1,
        ..Observation::default()
    };
    assert_eq!(
        producer.publish(&observation).err(),
        Some(ObsError::TooManyJoints)
    );
}

/// The geometry this format shares with the command ring is the same geometry.
///
/// Not imported from it — this crate takes no dependency on `skynet-ring`, because a ring is a place
/// two processes meet rather than a place another crate's opinions apply. So the constants are
/// declared twice and this is what keeps them equal, which is the trick `skynet-ring` itself uses to
/// hold `MAX_CHANNELS` against the kernel's. A comment saying they should agree would not.
#[test]
fn the_geometry_matches_the_command_rings() {
    // The literals are `skynet-ring`'s, written out rather than imported.
    assert_eq!(SLOT_BYTES, 4096, "one page, and the same page");
    assert_eq!(HEADER_BYTES, 4096);
    assert_eq!(
        MAX_JOINTS, 128,
        "a body with more joints than the command ring has channels could be observed and not \
         commanded, which is a robot this platform can describe and cannot drive",
    );
}

/// The slot's fields fit its page with room, and the arithmetic saying so is worth pinning.
#[test]
fn a_slot_holds_three_arrays_and_a_base_pose_with_room_left() {
    assert_eq!(SLOT_USED_WORDS, 22 + 3 * MAX_JOINTS);
    // `const` blocks rather than plain assertions: both operands are compile-time constants, so
    // a slot that no longer fits its page should fail to *build* rather than fail a test
    // somebody has to run. It is also what clippy asks for, and here the lint is right about
    // the mechanism.
    const {
        assert!(SLOT_USED_WORDS <= SLOT_WORDS);
    }
    const {
        assert!(
            SLOT_USED_WORDS + MAX_JOINTS > SLOT_WORDS || SLOT_USED_WORDS < SLOT_WORDS,
            "a fourth per-joint array is the next thing anybody will want to add, and this is \
             where they will find out whether it fits",
        );
    }
}

/// Reading twice without a publish in between gives the same observation, not an emptier one.
///
/// Compared field by field rather than with `assert_eq!` on the whole struct, and the reason is the
/// point of the case: `Observation` has no `PartialEq` because every joint above the declared count
/// is `NaN`, so a derived comparison would call two reads of one slot different.
#[test]
fn a_second_read_without_a_publish_repeats_the_last_observation() {
    let region = words();
    let ring = Ring::create(&region, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&region).expect("the same ring").consumer();

    producer.publish(&sample(2, 4_000, 7)).expect("published");

    let mut first = Observation::default();
    let mut second = Observation::default();
    assert_eq!(consumer.read(&mut first), Reading::Fresh);
    assert_eq!(consumer.read(&mut second), Reading::Fresh);

    assert_eq!(first.stamp_ns, second.stamp_ns);
    assert_eq!(first.seq, second.seq);
    assert_eq!(first.joints, second.joints);
    for joint in 0..first.joints {
        assert_eq!(
            first.position[joint].to_bits(),
            second.position[joint].to_bits(),
        );
    }
    assert_eq!(
        second.stamp_ns, 4_000,
        "and the stamp did not move — which is the only thing a policy can read here to \
         tell a body that has not moved from a producer that has stopped",
    );
}
