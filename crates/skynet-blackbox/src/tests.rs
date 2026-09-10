//! The black box's suite.
//!
//! Organised by the property being held rather than by module, because the properties are what §13
//! and §26 actually promise: the ring wraps without losing its shape, a torn record says it is
//! torn, a gap is visible as a gap, the sealed file is well-formed MCAP, and a recording replays.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use skynet_kernel::{
    resolve_limits, Available, Enforcement, Envelope, FaultResponse, Guard, ResolveLimitsParams,
    Tier, TransportClamp,
};

use crate::mcap;
use crate::record::{
    decode, encode, Body, CycleView, Entry, EventCode, EventView, FrameView, Gap, GapReason,
    ReadOutcome, RecordError, Shape, NO_CAPABILITY, RECORD_SCHEMA,
};
use crate::replay::{replay, replay_ring};
use crate::ring::{Geometry, Meta, Ring, RingError};
use crate::schema;
use crate::seal::{seal, Reason};

// ── Fixtures ─────────────────────────────────────────────────────────────────

const CHANNELS: [&str; 3] = [
    "right_arm.shoulder.pitch",
    "right_arm.elbow.pitch",
    "right_arm.wrist.rotation",
];

fn shape() -> Shape {
    Shape {
        joints: 3,
        channels: 3,
    }
}

fn meta() -> Meta {
    Meta {
        robot_id: "dev-0001".to_string(),
        embodiment: [0x11; 32],
        os_image: [0x22; 32],
        tier: 1,
        enforcement: 1,
        simulator: false,
        wall_epoch_ns: 1_700_000_000_000_000_000,
        mono_ns: 5_000_000_000,
        channels: CHANNELS.iter().map(|name| name.to_string()).collect(),
    }
}

fn ring_of(records: usize) -> (Ring, Vec<u8>) {
    let geometry = Geometry {
        shape: shape(),
        record_size: shape().record_size(),
        capacity: records,
        header_len: 4096,
        digests: 4,
        capabilities: 4,
    };
    Ring::create(geometry, meta())
}

/// A cycle where nothing tripped, parameterised by tick so a window has distinguishable records.
fn plain_cycle(tick: u64) -> OwnedCycle {
    OwnedCycle {
        tick,
        desired: vec![0.10, 0.20, 0.30],
        command: vec![0.10, 0.20, 0.30],
        mask: vec![1, 1, 1],
        clamped: vec![false, false, false],
        position: vec![0.09, 0.19, 0.29],
        velocity: vec![0.0, 0.0, f32::NAN],
        effort: vec![1.0, 2.0, f32::NAN],
        tripped: Vec::new(),
        response: None,
        read_result: 0,
    }
}

/// Owned so a test can build one and lend it to a [`CycleView`] without fighting lifetimes.
struct OwnedCycle {
    tick: u64,
    desired: Vec<f32>,
    command: Vec<f64>,
    mask: Vec<u8>,
    clamped: Vec<bool>,
    position: Vec<f32>,
    velocity: Vec<f32>,
    effort: Vec<f32>,
    tripped: Vec<Guard>,
    response: Option<FaultResponse>,
    /// `0` unless a case is about a read that failed.
    read_result: i32,
}

impl OwnedCycle {
    fn view(&self) -> CycleView<'_> {
        CycleView {
            tick: self.tick,
            observation: [0xAB; 32],
            policy: 0,
            state_flags: 0,
            fault_code: 0,
            rhal_result: 0,
            read_result: self.read_result,
            state_seq: self.tick as u32,
            state_stamp_ns: 1_000 + self.tick,
            command_stamp_ns: 1_000 + self.tick,
            command_seq: self.tick as u32,
            mode: 0,
            advisory: false,
            response: self.response,
            tripped: &self.tripped,
            capability: NO_CAPABILITY,
            nearest_person_m: f32::NAN,
            base_pose: [f32::NAN; 7],
            base_twist: [0.0; 6],
            desired: &self.desired,
            command: &self.command,
            mask: &self.mask,
            clamped: &self.clamped,
            position: &self.position,
            velocity: &self.velocity,
            effort: &self.effort,
        }
    }
}

// ── The record ───────────────────────────────────────────────────────────────

/// §24's read outcome, and the schema byte that makes a zero readable.
///
/// The whole point of the field is the case a recording could not previously express: a cycle in
/// which the driver did not answer, encoded with zeroed flags, a zero sequence and the previous
/// cycle's joint positions — a record indistinguishable from a healthy one.
#[test]
fn a_cycle_says_whether_the_read_succeeded() {
    let shape = Shape {
        joints: 2,
        channels: 2,
    };
    let mut slot = vec![0u8; shape.record_size()];

    let two = |tick: u64, read_result: i32| OwnedCycle {
        tick,
        desired: vec![0.10, 0.20],
        command: vec![0.10, 0.20],
        mask: vec![1, 1],
        clamped: vec![false, false],
        position: vec![0.09, 0.19],
        velocity: vec![0.0, f32::NAN],
        effort: vec![1.0, f32::NAN],
        tripped: Vec::new(),
        response: None,
        read_result,
    };

    let healthy = two(1, 0);
    encode(shape, &mut slot, 1, 100, Entry::Cycle(&healthy.view())).expect("a cycle");
    assert_eq!(
        slot[5], RECORD_SCHEMA,
        "records carry the schema they are in"
    );
    let Body::Cycle(decoded) = decode(shape, &slot).map(|r| r.body).expect("decodes") else {
        panic!("a cycle")
    };
    assert_eq!(decoded.read, ReadOutcome::Ok);
    assert!(decoded.read.measured());

    let timed_out = two(2, -110);
    encode(shape, &mut slot, 2, 200, Entry::Cycle(&timed_out.view())).expect("a cycle");
    let Body::Cycle(decoded) = decode(shape, &slot).map(|r| r.body).expect("decodes") else {
        panic!("a cycle")
    };
    assert_eq!(decoded.read, ReadOutcome::Failed(-110));
    assert!(
        !decoded.read.measured(),
        "the body fields in this record are fill values, and this is what says so"
    );

    // A record written before the field existed. Its 172..176 is reserved padding, so a reader must
    // answer *unrecorded* — never `Ok`. Reading absence as success is what made a robot whose
    // driver had stopped answering look healthy for as long as the recording lasted.
    encode(shape, &mut slot, 3, 300, Entry::Cycle(&healthy.view())).expect("a cycle");
    slot[5] = 1;
    let crc = crate::crc32::crc32(&slot[4..]);
    slot[0..4].copy_from_slice(&crc.to_le_bytes());
    let Body::Cycle(decoded) = decode(shape, &slot)
        .map(|r| r.body)
        .expect("an older record still decodes")
    else {
        panic!("a cycle")
    };
    assert_eq!(decoded.read, ReadOutcome::Unrecorded);
    assert!(!decoded.read.measured());
}

#[test]
fn a_cycle_survives_a_round_trip() {
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    let owned = plain_cycle(7);
    let view = owned.view();
    encode(shape, &mut slot, 42, 900, Entry::Cycle(&view)).unwrap();

    let record = decode(shape, &slot).unwrap();
    assert_eq!(record.sequence, 42);
    assert_eq!(record.mono_ns, 900);
    let Body::Cycle(cycle) = record.body else {
        panic!("not a cycle")
    };
    assert_eq!(cycle.tick, 7);
    assert_eq!(cycle.desired, vec![0.10f32, 0.20, 0.30]);
    assert_eq!(cycle.command, vec![0.10f64, 0.20, 0.30]);
    assert_eq!(cycle.mask, vec![1, 1, 1]);
    assert_eq!(cycle.position, vec![0.09f32, 0.19, 0.29]);
    // NaN means "this joint does not measure it" (§24) and must survive as NaN, not as zero.
    assert!(cycle.velocity[2].is_nan());
    assert!(cycle.effort[2].is_nan());
    assert!(cycle.base_pose.iter().all(|value| value.is_nan()));
}

#[test]
fn a_verdict_survives_a_round_trip() {
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    let mut owned = plain_cycle(1);
    owned.tripped = vec![Guard::Envelope, Guard::RateLimit];
    owned.response = Some(FaultResponse::Clamp);
    owned.clamped = vec![true, false, true];
    owned.command = vec![0.05, 0.20, 0.15];
    let view = owned.view();
    encode(shape, &mut slot, 1, 0, Entry::Cycle(&view)).unwrap();

    let Body::Cycle(cycle) = decode(shape, &slot).unwrap().body else {
        panic!("not a cycle")
    };
    assert_eq!(cycle.tripped, vec![Guard::Envelope, Guard::RateLimit]);
    assert_eq!(cycle.response, Some(FaultResponse::Clamp));
    assert_eq!(cycle.clamped, vec![true, false, true]);
}

#[test]
fn an_array_of_the_wrong_length_is_refused() {
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    let mut owned = plain_cycle(1);
    owned.desired = vec![0.1, 0.2];
    let view = owned.view();
    let error = encode(shape, &mut slot, 1, 0, Entry::Cycle(&view)).unwrap_err();
    assert_eq!(
        error,
        RecordError::Shape {
            field: "desired",
            expected: 3,
            found: 2
        }
    );
}

#[test]
fn a_torn_record_says_so() {
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    let owned = plain_cycle(1);
    let view = owned.view();
    encode(shape, &mut slot, 1, 0, Entry::Cycle(&view)).unwrap();
    // One byte, in the middle of the arrays. This is what a write interrupted by power loss looks
    // like from the reader's side, and the point of the CRC is that it does not look like a cycle.
    slot[200] ^= 0x01;
    assert_eq!(decode(shape, &slot), Err(RecordError::Torn));
}

#[test]
fn an_unwritten_slot_is_empty_rather_than_torn() {
    let shape = shape();
    let slot = vec![0u8; shape.record_size()];
    assert_eq!(decode(shape, &slot), Err(RecordError::Empty));
}

#[test]
fn the_four_kinds_fit_one_slot() {
    // The record size is the maximum over the kinds, so a small body's ring must still hold an
    // event — which is longer than that body's cycle.
    let narrow = Shape {
        joints: 1,
        channels: 1,
    };
    let mut slot = vec![0u8; narrow.record_size()];
    for entry in [
        Entry::Event(&EventView {
            code: EventCode::Estop,
            subject: "operator",
            detail: "the button on the chassis",
        }),
        Entry::Gap(Gap {
            dropped: 12,
            first_tick: 900,
            reason: GapReason::EncoderBehind,
        }),
        Entry::Frame(&FrameView {
            observation: [1; 32],
            target_ns: 42,
            worst_skew_us: Some(8_000),
            streams_omitted: 0,
            streams_absent: 0,
            streams: &[("wrist_cam", 31_000, false), ("joints", 900, false)],
        }),
    ] {
        let kind = entry.kind();
        encode(narrow, &mut slot, 1, 0, entry).unwrap();
        assert_eq!(decode(narrow, &slot).unwrap().body.kind(), kind);
    }
}

#[test]
fn a_frame_with_too_many_streams_counts_what_it_dropped() {
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    let streams: Vec<(&str, u32, bool)> = (0..9)
        .map(|index| {
            (
                ["a", "b", "c", "d", "e", "f", "g", "h", "i"][index],
                10,
                false,
            )
        })
        .collect();
    encode(
        shape,
        &mut slot,
        1,
        0,
        Entry::Frame(&FrameView {
            observation: [0; 32],
            target_ns: 0,
            worst_skew_us: Some(0),
            streams_omitted: 0,
            streams_absent: 0,
            streams: &streams,
        }),
    )
    .unwrap();
    let Body::Frame(frame) = decode(shape, &slot).unwrap().body else {
        panic!("not a frame")
    };
    assert_eq!(frame.streams.len(), 6);
    assert_eq!(frame.streams_omitted, 3);
}

#[test]
fn a_frame_with_no_measurable_skew_round_trips_as_none_and_not_as_zero() {
    // Format version 2's first reason. A frame with one contributing stream has no spread — the
    // spread of one instant is undefined — and version 1 had to write a number. Zero is the number
    // it would have written, and zero reads, months later, as a perfectly synchronised frame.
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    encode(
        shape,
        &mut slot,
        1,
        0,
        Entry::Frame(&FrameView {
            observation: [0; 32],
            target_ns: 7,
            worst_skew_us: None,
            streams_omitted: 0,
            streams_absent: 2,
            streams: &[("joints", 900, false)],
        }),
    )
    .unwrap();
    let Body::Frame(frame) = decode(shape, &slot).unwrap().body else {
        panic!("not a frame")
    };
    assert_eq!(frame.worst_skew_us, None);
    // And a real zero is still a zero. The sentinel is at the top of the range precisely so that
    // "no skew at all" and "no skew measurable" stay two different facts.
    let mut other = vec![0u8; shape.record_size()];
    encode(
        shape,
        &mut other,
        2,
        0,
        Entry::Frame(&FrameView {
            observation: [0; 32],
            target_ns: 7,
            worst_skew_us: Some(0),
            streams_omitted: 0,
            streams_absent: 0,
            streams: &[("a", 1, false), ("b", 1, false)],
        }),
    )
    .unwrap();
    let Body::Frame(frame) = decode(shape, &other).unwrap().body else {
        panic!("not a frame")
    };
    assert_eq!(frame.worst_skew_us, Some(0));
}

#[test]
fn a_dead_capture_path_is_not_a_record_that_ran_out_of_room() {
    // Format version 2's second reason, and the two counts must never be one. `streams_omitted` is
    // a limit of the format — more streams than the record carries — and `streams_absent` is a
    // limit of the robot. An investigation reading one as the other goes looking for a bigger
    // record instead of a dead camera.
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    let streams: Vec<(&str, u32, bool)> = ["a", "b", "c", "d", "e", "f", "g"]
        .iter()
        .map(|name| (*name, 10, false))
        .collect();
    encode(
        shape,
        &mut slot,
        1,
        0,
        Entry::Frame(&FrameView {
            observation: [0; 32],
            target_ns: 0,
            worst_skew_us: Some(5),
            streams_omitted: 0,
            streams_absent: 3,
            streams: &streams,
        }),
    )
    .unwrap();
    let Body::Frame(frame) = decode(shape, &slot).unwrap().body else {
        panic!("not a frame")
    };
    assert_eq!(frame.streams.len(), 6);
    // Seven offered, six written: one the record could not fit.
    assert_eq!(frame.streams_omitted, 1);
    // And three the robot never produced, which the record could have fitted perfectly well.
    assert_eq!(frame.streams_absent, 3);
}

#[test]
fn a_long_name_is_truncated_on_a_character_boundary() {
    let shape = shape();
    let mut slot = vec![0u8; shape.record_size()];
    // Seventeen bytes of two-byte characters into a sixteen-byte field: the cut must not land
    // inside a character, or the ring holds invalid UTF-8 and every reader has to guess.
    encode(
        shape,
        &mut slot,
        1,
        0,
        Entry::Frame(&FrameView {
            observation: [0; 32],
            target_ns: 0,
            worst_skew_us: Some(0),
            streams_omitted: 0,
            streams_absent: 0,
            streams: &[("ααααααααα", 1, false)],
        }),
    )
    .unwrap();
    let Body::Frame(frame) = decode(shape, &slot).unwrap().body else {
        panic!("not a frame")
    };
    assert_eq!(frame.streams[0].name, "αααααααα");
}

// ── The ring ─────────────────────────────────────────────────────────────────

#[test]
fn a_budget_buys_a_computable_number_of_seconds() {
    // The number §13's "roughly an hour" turns into for a body this size. Asserted so that a change
    // to the record layout shows up here as a changed window rather than as a surprise on a robot.
    let humanoid = Shape {
        joints: 30,
        channels: 30,
    };
    let geometry = Geometry::for_budget(humanoid, 16, 16, 512 * 1024 * 1024).unwrap();
    assert_eq!(geometry.record_size, 936);
    let seconds = geometry.seconds_covered(1000.0);
    assert!(
        (573.0..574.0).contains(&seconds),
        "512 MB at 1 kHz is {seconds} s, not the hour §13 asks for"
    );
    // And the other direction: what an hour would actually cost.
    let hour = Geometry::budget_for(humanoid, 16, 16, 1000.0, 3600.0);
    assert!(
        (3_300_000_000..3_400_000_000).contains(&hour),
        "an hour at 1 kHz is {hour} bytes"
    );
}

#[test]
fn a_budget_too_small_for_one_record_is_refused() {
    let error = Geometry::for_budget(shape(), 4, 4, 100).unwrap_err();
    assert!(matches!(error, RingError::TooSmall { budget: 100, .. }));
}

#[test]
fn the_ring_wraps_and_keeps_the_newest() {
    let (mut ring, mut store) = ring_of(4);
    for tick in 1..=6u64 {
        let owned = plain_cycle(tick);
        let view = owned.view();
        ring.append(&mut store, tick * 1000, Entry::Cycle(&view))
            .unwrap();
    }
    let held = ring.ordered(&store);
    assert_eq!(held.len(), 4);
    assert_eq!(held.first().unwrap().sequence, 3);
    assert_eq!(held.last().unwrap().sequence, 6);
}

#[test]
fn a_ring_reopens_where_it_left_off() {
    let (mut ring, mut store) = ring_of(8);
    for tick in 1..=5u64 {
        let owned = plain_cycle(tick);
        let view = owned.view();
        ring.append(&mut store, tick, Entry::Cycle(&view)).unwrap();
    }
    // What a restart does: nothing in memory survives, and the next sequence has to come off the
    // disk. The counter is not written anywhere, on purpose — see `Ring::open`.
    let reopened = Ring::open(&store).unwrap();
    assert_eq!(reopened.next_sequence(), 6);
    assert_eq!(reopened.meta().robot_id, "dev-0001");
    assert_eq!(reopened.meta().channels, CHANNELS.to_vec());
    assert_eq!(reopened.geometry(), ring.geometry());
}

#[test]
fn a_damaged_header_refuses_to_open() {
    let (_, mut store) = ring_of(4);
    store[100] ^= 0xFF;
    assert_eq!(Ring::open(&store).unwrap_err(), RingError::HeaderTorn);
}

#[test]
fn something_that_is_not_a_ring_refuses_to_open() {
    let store = vec![0u8; 8192];
    assert_eq!(Ring::open(&store).unwrap_err(), RingError::NotARing);
}

#[test]
fn interning_a_digest_is_idempotent_and_bounded() {
    let (mut ring, mut store) = ring_of(4);
    assert_eq!(ring.intern_digest(&mut store, [1; 32]).unwrap(), 0);
    assert_eq!(ring.intern_digest(&mut store, [2; 32]).unwrap(), 1);
    assert_eq!(ring.intern_digest(&mut store, [1; 32]).unwrap(), 0);
    assert_eq!(ring.intern_digest(&mut store, [3; 32]).unwrap(), 2);
    assert_eq!(ring.intern_digest(&mut store, [4; 32]).unwrap(), 3);
    assert_eq!(
        ring.intern_digest(&mut store, [5; 32]),
        Err(RingError::DigestTableFull)
    );
    // And it is durable: the table is in the header, so a restart still knows which policy.
    let reopened = Ring::open(&store).unwrap();
    assert_eq!(reopened.digests(), &[[1u8; 32], [2; 32], [3; 32], [4; 32]]);
}

#[test]
fn interning_a_capability_is_durable() {
    let (mut ring, mut store) = ring_of(4);
    assert_eq!(
        ring.intern_capability(&mut store, "manipulation.grasp")
            .unwrap(),
        0
    );
    assert_eq!(
        ring.intern_capability(&mut store, "locomotion.walk")
            .unwrap(),
        1
    );
    let reopened = Ring::open(&store).unwrap();
    assert_eq!(
        reopened.capabilities(),
        &["manipulation.grasp", "locomotion.walk"]
    );
}

#[test]
fn staging_says_where_the_record_goes() {
    // The file-backed path: one scratch buffer, one positioned write, no rewriting.
    let (mut ring, store) = ring_of(4);
    let mut slot = vec![0u8; ring.geometry().record_size];
    let owned = plain_cycle(1);
    let view = owned.view();
    let (sequence, offset) = ring.stage(&mut slot, 10, Entry::Cycle(&view)).unwrap();
    assert_eq!(sequence, 1);
    assert_eq!(offset, ring.geometry().header_len);
    // The fifth record of a four-slot ring goes back to the first slot.
    for _ in 0..3 {
        ring.stage(&mut slot, 10, Entry::Cycle(&view)).unwrap();
    }
    let (sequence, offset) = ring.stage(&mut slot, 10, Entry::Cycle(&view)).unwrap();
    assert_eq!(sequence, 5);
    assert_eq!(offset, ring.geometry().header_len);
    assert_eq!(store.len(), ring.geometry().total_bytes());
}

// ── Sealing ──────────────────────────────────────────────────────────────────

fn recorded_window() -> (Ring, Vec<u8>) {
    let (mut ring, mut store) = ring_of(16);
    ring.intern_digest(&mut store, [0x33; 32]).unwrap();
    ring.append(
        &mut store,
        1_000,
        Entry::Event(&EventView {
            code: EventCode::RecordingStarted,
            subject: "dev-0001",
            detail: "the ring was opened",
        }),
    )
    .unwrap();
    for tick in 1..=4u64 {
        let owned = plain_cycle(tick);
        let view = owned.view();
        ring.append(&mut store, 2_000 + tick * 1_000, Entry::Cycle(&view))
            .unwrap();
    }
    ring.append(
        &mut store,
        7_000,
        Entry::Frame(&FrameView {
            observation: [0xAB; 32],
            target_ns: 6_900,
            worst_skew_us: Some(12_000),
            streams_omitted: 0,
            streams_absent: 0,
            streams: &[("wrist_cam", 31_000, false), ("joints", 900, false)],
        }),
    )
    .unwrap();
    (ring, store)
}

#[test]
fn a_sealed_file_is_framed_mcap() {
    let (ring, store) = recorded_window();
    let sealed = seal(&ring, &store, Reason::Operator, 8_000);

    assert_eq!(&sealed.bytes[..8], &mcap::MAGIC);
    assert_eq!(&sealed.bytes[sealed.bytes.len() - 8..], &mcap::MAGIC);

    // Walk the record framing from the first opcode to the trailing magic. A file that does not
    // walk is a file no reader will open, and that is the whole reason §26 chose a format we do not
    // own.
    let mut at = 8;
    let mut opcodes = Vec::new();
    while at < sealed.bytes.len() - 8 {
        let opcode = sealed.bytes[at];
        let mut length = [0u8; 8];
        length.copy_from_slice(&sealed.bytes[at + 1..at + 9]);
        let length = u64::from_le_bytes(length) as usize;
        opcodes.push(opcode);
        at += 9 + length;
    }
    assert_eq!(
        at,
        sealed.bytes.len() - 8,
        "the framing did not land on the trailing magic"
    );
    assert_eq!(opcodes.first(), Some(&mcap::op::HEADER));
    assert_eq!(opcodes.last(), Some(&mcap::op::FOOTER));
    assert_eq!(
        opcodes
            .iter()
            .filter(|op| **op == mcap::op::DATA_END)
            .count(),
        1
    );
    // Six schemas and six channels in the data section, and the same again in the summary.
    assert_eq!(
        opcodes.iter().filter(|op| **op == mcap::op::SCHEMA).count(),
        12
    );
    assert_eq!(
        opcodes
            .iter()
            .filter(|op| **op == mcap::op::CHANNEL)
            .count(),
        12
    );
    assert_eq!(
        opcodes
            .iter()
            .filter(|op| **op == mcap::op::STATISTICS)
            .count(),
        1
    );
    assert_eq!(
        opcodes
            .iter()
            .filter(|op| **op == mcap::op::SUMMARY_OFFSET)
            .count(),
        3
    );
}

#[test]
fn the_footer_offsets_point_at_the_summary() {
    let (ring, store) = recorded_window();
    let sealed = seal(&ring, &store, Reason::Fault, 9_000);
    let end = sealed.bytes.len() - 8;
    let footer = &sealed.bytes[end - 29..end];
    assert_eq!(footer[0], mcap::op::FOOTER);
    let read = |at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&footer[at..at + 8]);
        u64::from_le_bytes(bytes) as usize
    };
    let summary_start = read(9);
    let summary_offset_start = read(17);
    assert!(summary_start > 0 && summary_start < summary_offset_start);
    assert_eq!(sealed.bytes[summary_start], mcap::op::SCHEMA);
    assert_eq!(sealed.bytes[summary_offset_start], mcap::op::SUMMARY_OFFSET);
}

#[test]
fn one_cycle_becomes_four_messages() {
    let (ring, store) = recorded_window();
    let sealed = seal(&ring, &store, Reason::Operator, 8_000);
    let text = String::from_utf8_lossy(&sealed.bytes);
    // Four cycles, so four of each of the cycle channels; and the frame and the two events.
    // Matched with the value's opening punctuation, so the two embedded copies of each schema —
    // which name every field twice, in `properties` and in `required` — are not counted as
    // messages. The first version of this assertion counted eight `position_rad`s and was right to.
    assert_eq!(text.matches("\"position_rad\":[").count(), 4);
    assert_eq!(text.matches("\"policy_digest\":\"").count(), 4);
    assert_eq!(text.matches("\"rhal_result\":0").count(), 4);
    assert_eq!(text.matches("\"worst_skew_us\":12000").count(), 1);
    assert!(text.contains("\"event\":\"recording_started\""));
    assert!(text.contains("\"event\":\"sealed\""));
}

#[test]
fn the_seal_carries_its_provenance() {
    let (ring, store) = recorded_window();
    let sealed = seal(&ring, &store, Reason::Estop, 8_000);
    let text = String::from_utf8_lossy(&sealed.bytes);
    assert!(text.contains("embodiment_digest"));
    assert!(text.contains(&mcap::hex(&[0x11u8; 32])));
    assert!(text.contains("sealed_reason"));
    assert!(text.contains("estop"));
    assert!(text.contains("policy_digest_0"));
    // §56.4's segment hash is the digest of the file, so two seals of the same window agree.
    let again = seal(&ring, &store, Reason::Estop, 8_000);
    assert_eq!(sealed.digest, again.digest);
}

#[test]
fn an_unmeasured_value_is_null_and_not_zero() {
    let (ring, store) = recorded_window();
    let sealed = seal(&ring, &store, Reason::Operator, 8_000);
    let text = String::from_utf8_lossy(&sealed.bytes);
    // The third joint reports no velocity and no effort, and the base is not localised.
    assert!(text.contains("\"velocity_rad_s\":[0,0,null]"));
    assert!(text.contains("\"base_pose\":[null,null,null,null,null,null,null]"));
}

#[test]
fn continuity_reports_a_complete_window() {
    let (ring, store) = recorded_window();
    let sealed = seal(&ring, &store, Reason::Operator, 8_000);
    assert!(sealed.continuity.complete());
    assert_eq!(sealed.continuity.records, 6);
    assert_eq!(sealed.continuity.first, 1);
    assert_eq!(sealed.continuity.last, 6);
    assert_eq!(sealed.continuity.cycles, 4);
    assert_eq!(sealed.continuity.interventions, 0);
}

#[test]
fn a_damaged_slot_makes_the_window_incomplete_without_refusing_to_seal() {
    let (ring, mut store) = recorded_window();
    // Corrupt the third record in place, as a bad sector would.
    let offset = ring.offset_of(3);
    store[offset + 50] ^= 0xFF;
    let sealed = seal(&ring, &store, Reason::Fault, 9_000);

    assert!(!sealed.continuity.complete());
    assert_eq!(sealed.continuity.damaged, 1);
    assert_eq!(
        sealed.continuity.holes,
        vec![crate::seal::Hole { from: 3, to: 3 }]
    );
    // And the file still exists, which is the point: the recording most likely to have a hole in it
    // is the one covering the fault that made the hole.
    assert!(!sealed.bytes.is_empty());
    // The metadata map holds bare strings rather than JSON, so the key is unquoted; the sealing
    // event's detail is where a reader sees it in prose.
    let text = String::from_utf8_lossy(&sealed.bytes);
    assert!(text.contains("complete"));
    assert!(text.contains("incomplete"));
}

#[test]
fn a_gap_is_recorded_as_a_gap() {
    let (mut ring, mut store) = ring_of(8);
    let owned = plain_cycle(1);
    let view = owned.view();
    ring.append(&mut store, 1_000, Entry::Cycle(&view)).unwrap();
    ring.append(
        &mut store,
        2_000,
        Entry::Gap(Gap {
            dropped: 150,
            first_tick: 2,
            reason: GapReason::EncoderBehind,
        }),
    )
    .unwrap();
    let sealed = seal(&ring, &store, Reason::Operator, 3_000);

    // The sequence numbers are dense — the gap record consumed one — so there is no *hole*, and yet
    // the window is not complete. That distinction is the whole of §47.4.
    assert!(sealed.continuity.holes.is_empty());
    assert_eq!(sealed.continuity.dropped, 150);
    assert!(!sealed.continuity.complete());
    let text = String::from_utf8_lossy(&sealed.bytes);
    assert!(text.contains("\"event\":\"gap\""));
    assert!(text.contains("\"reason\":\"encoder_behind\""));
}

#[test]
fn the_channel_schemas_are_stable() {
    // §31.2 keeps these single-source across editions. The corpus pins the bytes; this asserts the
    // shape of the set, so a channel added or renamed without a specification change fails here
    // first.
    let wire = schema::wire();
    let topics: Vec<&str> = wire.iter().map(|w| w.channel.topic).collect();
    assert_eq!(
        topics,
        vec![
            "/sk/state",
            "/sk/desired",
            "/sk/verdict",
            "/sk/command",
            "/sk/frame",
            "/sk/event"
        ]
    );
    for w in &wire {
        assert_eq!(w.channel.schema_id, w.schema.id);
        assert!(w.schema.definition.starts_with("{\"description\":"));
        assert!(w.schema.definition.contains("\"mono_ns\""));
    }
}

// ── Replay ───────────────────────────────────────────────────────────────────

/// A transport clamp with a per-channel ceiling on every channel.
///
/// The envelope's whole-body figure is deliberately *not* applied per channel by the kernel —
/// nothing at that layer knows whether a channel carries radians per second or newton-metres, so a
/// joint's real ceiling comes from the transport plan. A test that set only the envelope would
/// clamp nothing and would be asserting that replay agrees about a cycle in which the kernel never
/// had to decide anything.
fn clamp_at(ceiling: f64) -> [(&'static str, f64); 3] {
    // Sorted by name, which the kernel requires: elbow, then shoulder, then wrist.
    [
        (CHANNELS[1], ceiling),
        (CHANNELS[0], ceiling),
        (CHANNELS[2], ceiling),
    ]
}

fn limits_params<'a>(
    envelope: Envelope<'a>,
    per_channel: &'a [(&'a str, f64)],
) -> ResolveLimitsParams<'a> {
    ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope),
        transport: Some(TransportClamp {
            joint_velocity_max_rad_s: envelope.joint_velocity_max_rad_s,
            per_channel,
            ..TransportClamp::default()
        }),
        profile: None,
        // No frame graph, so the two pose guards report unavailable rather than running against a
        // body this suite has not described.
        frames: None,
        // A body that measures nothing beyond its joints: the guards needing a support polygon or a
        // person detector report unavailable, which is what §7.2 requires and what keeps this suite
        // about the black box rather than about the kernel.
        available: Available::default(),
    }
}

#[test]
fn a_recording_replays_against_the_kernel_that_made_it() {
    let envelope = Envelope {
        joint_velocity_max_rad_s: Some(1.0),
        ..Envelope::default()
    };
    let clamp = clamp_at(1.0);
    let params = limits_params(envelope, &clamp);
    let limits = resolve_limits(&params);
    assert_eq!(limits.enforcement, Enforcement::Enforcing);

    // Record what the kernel actually decides, cycle by cycle, exactly as a control loop would.
    let (mut ring, mut store) = ring_of(16);
    let mut previous: Option<Vec<(String, f64)>> = None;
    let mut last_sequence: Option<u64> = None;
    for tick in 1..=4u64 {
        // The fourth cycle asks for more than the envelope allows, so the kernel clamps.
        let asked = if tick == 4 { 2.5f32 } else { 0.4f32 };
        let desired: Vec<(&str, f64)> = CHANNELS.iter().map(|name| (*name, asked as f64)).collect();
        let carried: Option<Vec<(&str, f64)>> = previous.as_ref().map(|held| {
            held.iter()
                .map(|(name, value)| (name.as_str(), *value))
                .collect()
        });
        let verdict = skynet_kernel::evaluate_cycle(
            &skynet_kernel::CommandCycle {
                now_ms: tick as f64,
                command_ms: tick as f64,
                sequence: tick,
                desired: &desired,
                previous: carried.as_deref(),
                capability: None,
                nearest_person_m: None,
            },
            &limits,
            Some(&envelope),
            last_sequence,
        );

        let command: Vec<f64> = CHANNELS
            .iter()
            .map(|name| {
                verdict
                    .command
                    .iter()
                    .find(|(channel, _)| channel == name)
                    .map(|(_, value)| *value)
                    .unwrap_or(0.0)
            })
            .collect();
        let clamped: Vec<bool> = CHANNELS
            .iter()
            .map(|name| verdict.clamped.iter().any(|c| c.channel == *name))
            .collect();
        let tripped: Vec<Guard> = verdict.tripped.iter().copied().collect();

        let owned = OwnedCycle {
            tick,
            desired: vec![asked; 3],
            command: command.clone(),
            mask: vec![1, 1, 1],
            clamped,
            position: vec![0.0; 3],
            velocity: vec![0.0; 3],
            effort: vec![0.0; 3],
            tripped,
            response: verdict.response,
            read_result: 0,
        };
        let mut view = owned.view();
        view.command_stamp_ns = tick * 1_000_000;
        view.command_seq = tick as u32;
        ring.append(&mut store, tick * 1_000_000, Entry::Cycle(&view))
            .unwrap();

        previous = Some(
            CHANNELS
                .iter()
                .map(|name| name.to_string())
                .zip(command)
                .collect(),
        );
        last_sequence = Some(tick);
    }

    let result = replay_ring(&ring, &store, &limits, Some(&envelope));
    assert_eq!(result.cycles, 4);
    assert_eq!(result.agreed, 4, "diverged: {:?}", result.diverged);
    assert_eq!(result.warmed, 3, "the first cycle has no predecessor");
    assert!(result.deterministic());

    // The clamp really happened, so the test is not passing by never exercising one.
    let held = ring.ordered(&store);
    let Body::Cycle(last) = &held[3].body else {
        panic!("not a cycle")
    };
    assert_eq!(last.tripped, vec![Guard::Envelope]);
    assert!(last.clamped.iter().all(|was| *was));
}

#[test]
fn a_changed_limit_diverges_rather_than_agreeing_quietly() {
    // The same recording, replayed against a profile that was not in force. This is the case that
    // makes replay worth running: an incident read against the wrong configuration must say so.
    // Written by hand rather than recorded, because what it has to be is a recording made under a
    // ceiling of 1.0: a clamp to exactly that, on every channel.
    let (mut ring, mut store) = ring_of(8);
    let owned = OwnedCycle {
        tick: 1,
        desired: vec![2.5, 2.5, 2.5],
        command: vec![1.0, 1.0, 1.0],
        mask: vec![1, 1, 1],
        clamped: vec![true, true, true],
        position: vec![0.0; 3],
        velocity: vec![0.0; 3],
        effort: vec![0.0; 3],
        tripped: vec![Guard::Envelope],
        response: Some(FaultResponse::Clamp),
        read_result: 0,
    };
    let view = owned.view();
    ring.append(&mut store, 1_000_000, Entry::Cycle(&view))
        .unwrap();

    let tighter = Envelope {
        joint_velocity_max_rad_s: Some(0.5),
        ..Envelope::default()
    };
    let tighter_clamp = clamp_at(0.5);
    let tighter_params = limits_params(tighter, &tighter_clamp);
    let tighter_limits = resolve_limits(&tighter_params);
    let result = replay_ring(&ring, &store, &tighter_limits, Some(&tighter));

    assert!(!result.deterministic());
    assert_eq!(result.diverged.len(), 1);
    let fields: Vec<&str> = result.diverged[0]
        .divergences
        .iter()
        .map(|d| d.field)
        .collect();
    assert!(fields.contains(&"command"), "{fields:?}");
}

/// §26.4's other half: a window that reproduces perfectly and was not measured.
///
/// The cycle whose `read_state` failed replays **exactly** — the kernel's verdict comes from the
/// recorded desired values, timing and limits, and the read supplies none of them — so a replay
/// that skipped it or called it a divergence would be reporting non-determinism on the strength of
/// something determinism does not depend on. What it must not do is stay silent: every body field
/// in that record is a fill value holding the previous cycle's reading, so an investigator reading
/// "the arm was stationary" out of it is reading the same numbers twice.
///
/// So: deterministic **and** not measured, said separately, with the driver's own code.
#[test]
fn a_failed_read_replays_exactly_and_is_still_marked_as_unmeasured() {
    let (mut ring, mut store) = ring_of(8);
    let params = limits_params(Envelope::default(), &[]);
    let limits = resolve_limits(&params);

    for (tick, read_result) in [(1u64, 0i32), (2, -110), (3, 0)] {
        let owned = OwnedCycle {
            tick,
            desired: vec![0.25, 0.25, 0.25],
            command: vec![0.25, 0.25, 0.25],
            mask: vec![1, 1, 1],
            clamped: vec![false, false, false],
            position: vec![0.0; 3],
            velocity: vec![0.0; 3],
            effort: vec![0.0; 3],
            tripped: Vec::new(),
            response: None,
            read_result,
        };
        let mut view = owned.view();
        view.command_stamp_ns = tick * 1_000_000;
        view.command_seq = tick as u32;
        ring.append(&mut store, tick * 1_000_000, Entry::Cycle(&view))
            .unwrap();
    }

    let result = replay_ring(&ring, &store, &limits, None);

    assert_eq!(result.cycles, 3);
    assert_eq!(result.agreed, 3, "diverged: {:?}", result.diverged);
    assert!(
        result.deterministic(),
        "a failed read does not make a window non-deterministic"
    );

    // And the half the pass must not swallow.
    assert!(!result.observations_measured());
    assert_eq!(result.unmeasured, 1);
    assert_eq!(result.unrecorded, 0);
    assert_eq!(
        result.read_errors,
        vec![-110],
        "the driver's own code, carried verbatim (§24)"
    );

    let note = result
        .observation_note()
        .expect("a window with a fill says so");
    assert!(
        note.contains("1 of 3 cycles carry no measurement"),
        "{note}"
    );
    assert!(
        note.contains("-110"),
        "the code reaches the sentence: {note}"
    );
}

/// A recording written before the read's outcome existed is not a recording of successful reads.
///
/// §26.4: *"a reader that ignored the byte would read every older recording as a robot whose every
/// read succeeded, which is precisely the failure the field exists to prevent."* The replay reports
/// those cycles as unrecorded — a third answer, because reporting them as fills would invent a
/// fault in a window that may have had none.
#[test]
fn a_schema_one_recording_is_unrecorded_rather_than_measured() {
    let (mut ring, mut store) = ring_of(8);
    let params = limits_params(Envelope::default(), &[]);
    let limits = resolve_limits(&params);

    let owned = OwnedCycle {
        tick: 1,
        desired: vec![0.25, 0.25, 0.25],
        command: vec![0.25, 0.25, 0.25],
        mask: vec![1, 1, 1],
        clamped: vec![false, false, false],
        position: vec![0.0; 3],
        velocity: vec![0.0; 3],
        effort: vec![0.0; 3],
        tripped: Vec::new(),
        response: None,
        read_result: 0,
    };
    let mut view = owned.view();
    view.command_stamp_ns = 1_000_000;
    view.command_seq = 1;
    ring.append(&mut store, 1_000_000, Entry::Cycle(&view))
        .unwrap();

    // The decoded shape a schema-1 slot produces, reached by decoding a real record and setting the
    // one field the byte decides. Flipping the byte in the store would be the more literal fixture
    // and would test the wrong thing: the slot carries a CRC, so a hand-edited byte decodes as
    // `Torn` and the record never reaches `replay` at all. What is under test here is what replay
    // does with `ReadOutcome::Unrecorded`; `record.rs`'s own tests cover the byte producing it.
    let mut records = ring.ordered(&store);
    let Body::Cycle(cycle) = &mut records[0].body else {
        panic!("not a cycle")
    };
    cycle.read = ReadOutcome::Unrecorded;

    let result = replay(
        &records,
        &ring.meta().channels,
        ring.capabilities(),
        &limits,
        None,
    );
    assert_eq!(result.cycles, 1);
    assert_eq!(result.unmeasured, 0, "nothing here says a read failed");
    assert_eq!(result.unrecorded, 1);
    assert!(!result.observations_measured());
    let note = result
        .observation_note()
        .expect("a schema-1 window says so");
    assert!(note.contains("schema 1"), "{note}");
    assert!(
        note.contains("not the same as their being fine"),
        "the note refuses the reading §26.4 exists to prevent: {note}"
    );
}

#[test]
fn a_window_with_no_cycles_is_not_deterministic() {
    // Vacuous truth is the failure mode of every conformance predicate. An empty replay reports
    // false, in the same spirit as `field_conformance` returning undefined for zero cycles.
    let (ring, store) = ring_of(4);
    assert!(!store.is_empty());
    let params = limits_params(Envelope::default(), &[]);
    let limits = resolve_limits(&params);
    let result = replay(
        &[],
        &ring.meta().channels,
        ring.capabilities(),
        &limits,
        None,
    );
    assert_eq!(result.cycles, 0);
    assert!(!result.deterministic());
}

// ── §74's counters, off the ring ─────────────────────────────────────────────

/// A cycle in which `tripped` guards fired and `clamped` channels were clamped.
fn eventful_cycle(tick: u64, tripped: Vec<Guard>, clamped: Vec<bool>) -> OwnedCycle {
    OwnedCycle {
        tripped,
        clamped,
        ..plain_cycle(tick)
    }
}

#[test]
fn a_tally_counts_guards_per_trip_and_interventions_per_cycle() {
    // The distinction the two counters exist for. One cycle in which three guards fire is *one*
    // intervention and *three* trips, and a console dividing by both draws two different lines
    // rather than one line twice.
    let (mut ring, mut store) = ring_of(16);
    for cycle in [
        plain_cycle(1),
        eventful_cycle(
            2,
            vec![Guard::Envelope, Guard::RateLimit, Guard::Watchdog],
            vec![true, true, false],
        ),
        eventful_cycle(3, vec![Guard::Envelope], vec![false, false, true]),
        plain_cycle(4),
    ] {
        let view = cycle.view();
        ring.append(&mut store, cycle.tick * 1_000, Entry::Cycle(&view))
            .unwrap();
    }

    let tally = crate::tally::tally(&ring, &store, 0);
    assert_eq!(tally.cycles, 4);
    assert_eq!(tally.interventions, 2, "two cycles had a guard trip");
    assert_eq!(
        tally.guard_trips, 4,
        "three guards in one cycle and one in another"
    );
    assert_eq!(tally.clamps, 3, "two channels clamped, then one");
    assert!(tally.measured(), "{tally:?}");
}

#[test]
fn a_tally_covers_the_window_and_not_the_recording() {
    // §74's window is the interval between two reconcile passes; the ring is minutes or years.
    // A tally that counted the whole ring would report a robot's entire life every five minutes.
    let (mut ring, mut store) = ring_of(16);
    for tick in 1..=3 {
        let cycle = eventful_cycle(tick, vec![Guard::Envelope], vec![true, false, false]);
        let view = cycle.view();
        ring.append(&mut store, tick * 1_000, Entry::Cycle(&view))
            .unwrap();
    }

    // The window opens here.
    let opened = ring.next_sequence();

    for tick in 4..=5 {
        let cycle = eventful_cycle(tick, vec![Guard::Envelope], vec![true, false, false]);
        let view = cycle.view();
        ring.append(&mut store, tick * 1_000, Entry::Cycle(&view))
            .unwrap();
    }

    let tally = crate::tally::tally(&ring, &store, opened);
    assert_eq!(tally.cycles, 2, "the three before the window are not in it");
    assert_eq!(tally.interventions, 2);
    assert_eq!(tally.guard_trips, 2);
    assert!(tally.measured());
}

#[test]
fn a_window_the_ring_wrapped_over_is_truncated_rather_than_small() {
    // The failure this whole type exists to make impossible. A ring that overwrote the window's own
    // records still yields a number, and that number is a floor presented as a count — §47.4's
    // "gap that looks like a quiet period", in the one place §74.1 chose its shape to avoid it.
    let (mut ring, mut store) = ring_of(4);
    let opened = ring.next_sequence();
    for tick in 1..=10 {
        let cycle = eventful_cycle(tick, vec![Guard::Envelope], vec![true, false, false]);
        let view = cycle.view();
        ring.append(&mut store, tick * 1_000, Entry::Cycle(&view))
            .unwrap();
    }

    let tally = crate::tally::tally(&ring, &store, opened);
    assert!(tally.truncated, "six of the ten were overwritten");
    assert!(!tally.complete());
    assert!(
        !tally.measured(),
        "and so the counts must not be reported: {tally:?}"
    );
    // The counts are still there — they are a floor, and the caller is told not to trust them
    // rather than handed a zero it cannot distinguish from a quiet robot.
    assert_eq!(tally.cycles, 4);
}

#[test]
fn a_window_that_fits_is_not_truncated() {
    // The boundary the truncation test needs beside it: the ring holding exactly the window is the
    // ordinary case, and reporting it as lost would make every counter permanently unmeasured.
    let (mut ring, mut store) = ring_of(4);
    let opened = ring.next_sequence();
    for tick in 1..=4 {
        let cycle = plain_cycle(tick);
        let view = cycle.view();
        ring.append(&mut store, tick * 1_000, Entry::Cycle(&view))
            .unwrap();
    }

    let tally = crate::tally::tally(&ring, &store, opened);
    assert!(!tally.truncated, "{tally:?}");
    assert_eq!(tally.cycles, 4);
    assert!(tally.measured());
}

#[test]
fn a_ring_with_no_channels_reports_unmeasured_rather_than_zero() {
    // §74.3, and the reason this is a property of the *ring* rather than of the edition: a ring
    // created with no action channels is a robot with no command path, so zero cycles is the
    // absence of a producer and not a measurement. The day a control loop opens a ring with
    // channels in it, these counters begin reporting with no change here.
    let empty = Shape {
        joints: 0,
        channels: 0,
    };
    let geometry = Geometry {
        shape: empty,
        record_size: empty.record_size(),
        capacity: 8,
        header_len: 4096,
        digests: 4,
        capabilities: 4,
    };
    let (mut ring, mut store) = Ring::create(geometry, meta());
    ring.append(
        &mut store,
        1_000,
        Entry::Event(&EventView {
            code: EventCode::RecordingStarted,
            subject: "skynetd",
            detail: "started",
        }),
    )
    .unwrap();

    let tally = crate::tally::tally(&ring, &store, 0);
    assert_eq!(tally.cycles, 0);
    assert!(!tally.commands);
    assert!(
        !tally.measured(),
        "zero cycles here is the absence of a control loop, not a measurement"
    );
    // And it is complete: nothing was lost. The two reasons a tally cannot be reported are
    // separate facts, and collapsing them would make a wrapped ring indistinguishable from a
    // robot that never commanded anything.
    assert!(tally.complete());
}

#[test]
fn an_estop_is_counted_where_it_is_recorded_and_a_clearance_is_not() {
    // §26 puts an estop on the event channel, which is where it is counted from. Counting the
    // clearance too would report two for one stop, and make the rate depend on whether an operator
    // had got round to clearing it.
    let (mut ring, mut store) = ring_of(8);
    let opened = ring.next_sequence();
    for (code, detail) in [
        (EventCode::Estop, "button"),
        (EventCode::EstopCleared, "operator"),
        (EventCode::Estop, "button"),
    ] {
        ring.append(
            &mut store,
            1_000,
            Entry::Event(&EventView {
                code,
                subject: "robot",
                detail,
            }),
        )
        .unwrap();
    }

    let tally = crate::tally::tally(&ring, &store, opened);
    assert_eq!(tally.estops, 2);
    assert_eq!(tally.cycles, 0);
}

#[test]
fn a_gap_inside_the_window_makes_it_unreportable() {
    // §47.4: the producer saying what it lost is the only reason the loss is visible. A window
    // holding a gap is a window whose counts are a floor, so it is reported as unmeasured — and
    // the Hub sees a window it does not have rather than a small number it believes.
    let (mut ring, mut store) = ring_of(8);
    let opened = ring.next_sequence();
    let cycle = plain_cycle(1);
    let view = cycle.view();
    ring.append(&mut store, 1_000, Entry::Cycle(&view)).unwrap();
    ring.append(
        &mut store,
        2_000,
        Entry::Gap(Gap {
            dropped: 400,
            first_tick: 2,
            reason: GapReason::WriterBehind,
        }),
    )
    .unwrap();

    let tally = crate::tally::tally(&ring, &store, opened);
    assert_eq!(tally.cycles, 1);
    assert_eq!(tally.dropped, 400);
    assert!(!tally.complete());
    assert!(!tally.measured());
}

// ── §26.2's claimed sequences ────────────────────────────────────────────────
//
// `stage_claimed` is the append two processes share. The properties it has to have are the ones a
// single-writer ring gets for free from its own counter, so they are worth asserting where they can
// be seen: the slot a record lands in, what the ring's own counter does afterwards, and the one
// value the arithmetic cannot take.

/// Two writers interleaving claimed sequences produce one ordered recording, and every record is
/// in the slot its sequence names.
#[test]
fn records_staged_at_claimed_sequences_land_where_their_sequence_says() {
    let (mut ring, mut store) = ring_of(8);
    let mut scratch = vec![0u8; ring.geometry().record_size];

    // What the two processes do: one claims 1 and 3, the other claims 2 and 4, and neither knows
    // the other exists. The order they are *written* in is not the order they are numbered in,
    // which is exactly the case a shared counter has to survive.
    for sequence in [1u64, 3, 2, 4] {
        let cycle = plain_cycle(sequence);
        let view = cycle.view();
        let offset = ring
            .stage_claimed(
                &mut scratch,
                1_000 * sequence,
                Entry::Cycle(&view),
                sequence,
            )
            .expect("a claimed sequence stages");
        let size = ring.geometry().record_size;
        store[offset..offset + size].copy_from_slice(&scratch);
    }

    let ordered = ring.ordered(&store);
    assert_eq!(
        ordered.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3, 4],
        "the file is read in sequence order, whatever order it was written in",
    );
    for record in &ordered {
        assert_eq!(
            record.mono_ns,
            1_000 * record.sequence,
            "record {} holds the stamp staged with it, so no two writers overwrote one slot",
            record.sequence,
        );
    }
}

/// The ring's own counter is raised, never lowered — so `next_sequence()` is a floor and not a
/// claim about what the other writer has taken.
#[test]
fn a_claimed_sequence_raises_the_rings_counter_and_never_lowers_it() {
    let (mut ring, _store) = ring_of(8);
    let mut scratch = vec![0u8; ring.geometry().record_size];
    let cycle = plain_cycle(1);
    let view = cycle.view();

    ring.stage_claimed(&mut scratch, 1_000, Entry::Cycle(&view), 5)
        .expect("staged");
    assert_eq!(
        ring.next_sequence(),
        6,
        "staging 5 means the recording holds at least five records",
    );

    // The other writer claimed 6 and 7 while this one was idle; this one's next claim is 8.
    ring.stage_claimed(&mut scratch, 2_000, Entry::Cycle(&view), 8)
        .expect("staged");
    assert_eq!(ring.next_sequence(), 9);

    // A late write of an *earlier* sequence — which cannot happen through a claim counter, and is
    // asserted anyway because the counter is what the ring reports to §74's telemetry window.
    ring.stage_claimed(&mut scratch, 3_000, Entry::Cycle(&view), 2)
        .expect("staged");
    assert_eq!(
        ring.next_sequence(),
        9,
        "the floor does not fall, or a window opened at 9 would be told it opened at 3",
    );
}

/// Zero is the one number the arithmetic cannot take, and it is exactly what an unseeded claim
/// counter hands out.
#[test]
fn a_zero_sequence_is_refused_rather_than_wrapped_onto_the_last_slot() {
    let (mut ring, _store) = ring_of(8);
    let mut scratch = vec![0u8; ring.geometry().record_size];
    let cycle = plain_cycle(0);
    let view = cycle.view();

    // `offset_of` is `(sequence - 1) % capacity`. A zero underflows it, and in a release build with
    // `overflow-checks` off would address the last slot in the ring — on top of the oldest record
    // this window holds, silently.
    match ring.stage_claimed(&mut scratch, 1_000, Entry::Cycle(&view), 0) {
        Err(crate::ring::RingError::Unseeded) => {}
        Err(other) => panic!("refused for the wrong reason: {other:?}"),
        Ok(_) => panic!("a zero sequence was accepted, which is a record written over another one"),
    }
}

// ── Reading a sealed recording back (§13, §41.3) ─────────────────────────────
//
// The seal has written MCAP since the sixth session and nothing has ever read one. These are the
// round trip: what `seal` wrote is what `read` finds, and a file that has been damaged in each of
// the ways a transferred file is damaged is refused by name rather than misread.

/// A window with events, a gap and cycles in it, sealed.
fn sealed_bytes() -> Vec<u8> {
    use crate::record::{Gap, GapReason};

    let (mut ring, mut store) = ring_of(16);

    ring.append(
        &mut store,
        1_000,
        Entry::Event(&EventView {
            code: EventCode::RecordingStarted,
            subject: "skynetd",
            detail: "ring opened",
        }),
    )
    .expect("an event");
    for tick in 1..=3u64 {
        let cycle = plain_cycle(tick);
        ring.append(&mut store, 2_000 + tick, Entry::Cycle(&cycle.view()))
            .expect("a cycle");
    }
    ring.append(
        &mut store,
        6_000,
        Entry::Gap(Gap {
            dropped: 41,
            first_tick: 900,
            reason: GapReason::WriterBehind,
        }),
    )
    .expect("a gap");

    crate::seal::seal(&ring, &store, crate::seal::Reason::Operator, 9_000).bytes
}

#[test]
fn what_the_seal_wrote_is_what_the_reader_finds() {
    let bytes = sealed_bytes();
    let recording = crate::read::read(&bytes).expect("a sealed window is readable");

    // §56.2's provenance, in the file rather than beside it. A recording that arrives without its
    // embodiment digest cannot be replayed against anything.
    assert_eq!(recording.meta("robot_id"), Some("dev-0001"));
    assert_eq!(recording.meta("sealed_reason"), Some("operator"));
    assert_eq!(recording.meta("simulator"), Some("false"));
    assert!(recording
        .meta("embodiment_digest")
        .is_some_and(|d| d.len() == 64));

    // The topics, so a message can be named rather than numbered.
    assert_eq!(recording.topic(crate::schema::id::EVENT), Some("/sk/event"));
    assert_eq!(
        recording.topic(crate::schema::id::COMMAND),
        Some("/sk/command")
    );

    // **Once each.** MCAP repeats the channels in the summary, so a reader that pushed every
    // `Channel` record would report every topic twice — which is what the first version did, and
    // what `skynet blackbox inspect` printed until a real recording was put through it.
    let mut ids: Vec<u16> = recording.channels.iter().map(|(id, _)| *id).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before, "a channel is declared once to a reader");

    // An empty metadata value reads as absent rather than as a blank fact: the seal writes every
    // key whether or not the robot had something to put in it.
    assert_eq!(
        recording.meta("holes"),
        None,
        "no holes is not a hole list of one empty string"
    );

    // Three cycles, each written as four messages, plus the opening event, the gap, and the seal's
    // own closing event. The counts are asserted per channel because a reader that dropped one
    // channel entirely would still produce a plausible total.
    assert_eq!(recording.on(crate::schema::id::STATE).count(), 3);
    assert_eq!(recording.on(crate::schema::id::COMMAND).count(), 3);
    assert_eq!(
        recording.on(crate::schema::id::EVENT).count(),
        3,
        "start, gap, seal"
    );

    // The payloads are the JSON the seal wrote, not bytes the reader guessed at.
    let last = recording
        .on(crate::schema::id::EVENT)
        .last()
        .expect("the seal's own event");
    assert!(last.payload.contains("\"sealed\""), "{}", last.payload);
}

#[test]
fn a_file_that_is_not_a_recording_says_so_rather_than_being_parsed() {
    // The first thing anybody does with a reader is point it at the wrong file.
    match crate::read::read(b"{\"not\":\"mcap\"}") {
        Err(crate::read::ReadError::NotMcap) => {}
        other => panic!("{other:?}"),
    }
    match crate::read::read(&[]) {
        Err(crate::read::ReadError::NotMcap) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_transfer_that_stopped_is_a_different_answer_from_a_file_that_is_damaged() {
    // The distinction sends somebody somewhere different: a truncated file is a transfer to resume
    // (§79), and a damaged one is a file to replace. A reader that called both "bad file" would
    // hide the one that has a remedy.
    let bytes = sealed_bytes();

    let cut = &bytes[..bytes.len() - 200];
    match crate::read::read(cut) {
        Err(crate::read::ReadError::Truncated { .. }) => {}
        other => panic!("a cut file should read as truncated: {other:?}"),
    }

    // A length field that claims more than the file holds. Overwritten in place so the magic at
    // both ends still matches — which is what makes this "damaged" rather than "truncated".
    let mut damaged = bytes.clone();
    let length_at = crate::mcap::MAGIC.len() + 1;
    damaged[length_at..length_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    match crate::read::read(&damaged) {
        Err(crate::read::ReadError::Overrun { .. }) => {}
        other => panic!("a bad length should read as an overrun: {other:?}"),
    }
}

#[test]
fn every_refusal_has_a_sentence_naming_what_it_is_about() {
    // §64: a refusal that does not say what to do about it is a refusal somebody re-runs.
    for error in [
        crate::read::ReadError::NotMcap,
        crate::read::ReadError::Truncated { at: 12 },
        crate::read::ReadError::Overrun {
            opcode: 5,
            at: 9,
            wanted: 1 << 40,
        },
        crate::read::ReadError::Malformed {
            opcode: 12,
            field: "key",
        },
    ] {
        let detail = error.detail();
        assert!(detail.len() > 40, "{error:?} says too little: {detail}");
        assert!(
            detail.ends_with('.'),
            "{error:?} is a sentence and should end like one: {detail}"
        );
    }
}

// ── Reassembly, and replaying a window that has left the robot (§13, §83.5) ──
//
// `replay` was written to take records and name tables rather than a `Ring` — its own doc says so,
// "so that a recording read from a sealed file … replays through the same function". Nothing did.
// These are that join, and the assertion that matters is the one comparing the two: a replay off
// the file and a replay off the ring must reach the same verdict about the same window, or the
// seal has lost something the kernel evaluates.

/// A window recorded the way a control loop records one, returned as both a ring and a sealed file.
///
/// The capability token and the proximity reading are the point. They are the two inputs the kernel
/// *evaluates* that reached the ring and did not reach the file until 2026-09-09, and a fixture
/// that left them at their defaults would pass whether or not the seal writes them.
fn window_with_a_capability_and_a_person(ceiling: f64) -> (Ring, Vec<u8>, Vec<u8>) {
    let (mut ring, mut store) = ring_of(16);
    let token = ring
        .intern_capability(&mut store, "arm.move")
        .expect("a capability token interns");

    for tick in 1..=4u64 {
        let asked = if tick == 4 { 2.5f32 } else { 0.4f32 };
        let owned = OwnedCycle {
            tick,
            desired: vec![asked; 3],
            command: vec![asked.min(ceiling as f32) as f64; 3],
            mask: vec![1, 1, 1],
            clamped: vec![tick == 4; 3],
            position: vec![0.09, 0.19, f32::NAN],
            velocity: vec![0.0, 0.0, f32::NAN],
            effort: vec![1.0, 2.0, f32::NAN],
            tripped: if tick == 4 {
                vec![Guard::Envelope]
            } else {
                vec![]
            },
            response: if tick == 4 {
                Some(FaultResponse::Clamp)
            } else {
                None
            },
            read_result: if tick == 3 { -110 } else { 0 },
        };
        let mut view = owned.view();
        view.command_stamp_ns = tick * 1_000_000;
        view.command_seq = tick as u32;
        view.capability = token;
        // A person, closer each cycle. `NaN` on one of them, because "nothing was measuring it"
        // has to survive the round trip as itself rather than as a number.
        view.nearest_person_m = if tick == 2 {
            f32::NAN
        } else {
            3.0 - tick as f32
        };
        ring.append(&mut store, tick * 1_000_000, Entry::Cycle(&view))
            .expect("a cycle");
    }

    let bytes = crate::seal::seal(&ring, &store, crate::seal::Reason::Operator, 9_000).bytes;
    (ring, store, bytes)
}

#[test]
fn a_sealed_window_reassembles_into_the_cycles_it_was_written_from() {
    let (ring, store, bytes) = window_with_a_capability_and_a_person(1.0);
    let recording = crate::read::read(&bytes).expect("a sealed window is readable");
    let put_back = crate::assemble::assemble(&recording).expect("it reassembles");

    assert_eq!(put_back.records.len(), 4);
    assert!(put_back.incomplete.is_empty(), "{:?}", put_back.incomplete);

    // The file carried every field a replay evaluates. This is the assertion that fails the day
    // somebody removes one from the seal.
    assert_eq!(put_back.absent, crate::assemble::Absent::default());
    assert!(put_back.absent.verdict_inputs_all_recorded());

    // The channel table came out of the values arrays rather than out of a ring nobody has.
    let names: Vec<String> = CHANNELS.iter().map(|name| name.to_string()).collect();
    assert_eq!(put_back.channels, names);
    assert_eq!(put_back.capabilities, vec![String::from("arm.move")]);

    // Field by field against the ring, on the cycle where every interesting one is set.
    let held = ring.ordered(&store);
    for (from_ring, from_file) in held.iter().zip(put_back.records.iter()) {
        let (Body::Cycle(was), Body::Cycle(now)) = (&from_ring.body, &from_file.body) else {
            panic!("both are cycles")
        };
        assert_eq!(was.tick, now.tick);
        assert_eq!(was.tripped, now.tripped);
        assert_eq!(was.response, now.response);
        assert_eq!(was.advisory, now.advisory);
        assert_eq!(was.clamped, now.clamped);
        assert_eq!(was.command_seq, now.command_seq);
        assert_eq!(was.command_stamp_ns, now.command_stamp_ns);
        assert_eq!(was.read, now.read, "§26.4's outcome survives the seal");
        // Bit equality, because `NaN` is a value here rather than an error, and the cycle that
        // carries one is the cycle testing that "nothing measured it" is not read as a number.
        assert_eq!(
            was.nearest_person_m.to_bits(),
            now.nearest_person_m.to_bits()
        );
        // The index may differ — the file's table is built in first-seen order — so what has to
        // match is the token it resolves to.
        assert_eq!(
            ring.capabilities().get(was.capability as usize),
            put_back.capabilities.get(now.capability as usize)
        );
    }
}

#[test]
fn a_replay_off_the_file_reaches_the_same_verdict_as_a_replay_off_the_ring() {
    let ceiling = 1.0;
    let envelope = Envelope {
        joint_velocity_max_rad_s: Some(ceiling),
        ..Envelope::default()
    };
    let clamp = clamp_at(ceiling);
    let params = limits_params(envelope, &clamp);
    let limits = resolve_limits(&params);

    let (ring, store, bytes) = window_with_a_capability_and_a_person(ceiling);
    let from_ring = replay_ring(&ring, &store, &limits, Some(&envelope));

    let recording = crate::read::read(&bytes).expect("a sealed window is readable");
    let put_back = crate::assemble::assemble(&recording).expect("it reassembles");
    let from_file = replay(
        &put_back.records,
        &put_back.channels,
        &put_back.capabilities,
        &limits,
        Some(&envelope),
    );

    // The whole point. Two replays of one window, one off the ring it was written to and one off
    // the file it was sealed into, and every figure the same.
    assert_eq!(from_file.cycles, from_ring.cycles);
    assert_eq!(from_file.agreed, from_ring.agreed);
    assert_eq!(from_file.warmed, from_ring.warmed);
    assert_eq!(from_file.diverged.len(), from_ring.diverged.len());
    assert_eq!(from_file.unmeasured, from_ring.unmeasured);
    assert_eq!(from_file.read_errors, from_ring.read_errors);
    assert_eq!(from_file.deterministic(), from_ring.deterministic());

    // And it is not passing by being empty on both sides.
    assert_eq!(from_ring.cycles, 4);
    assert_eq!(
        from_ring.unmeasured, 1,
        "the third cycle's read failed and the file has to say so"
    );
}

#[test]
fn a_recording_sealed_before_the_fields_existed_is_counted_rather_than_guessed() {
    // The same window with the three keys stripped out of the payloads, which is what a file
    // sealed by an older edition looks like. It must still assemble — a reader that refused would
    // make every older recording unopenable — and it must say, per field, how many cycles could
    // not supply it, so nothing downstream reports a verdict comparison as though the inputs were
    // known.
    let (_, _, bytes) = window_with_a_capability_and_a_person(1.0);
    let mut recording = crate::read::read(&bytes).expect("a sealed window is readable");
    for message in &mut recording.messages {
        for key in ["nearest_person_m", "read_result", "capability"] {
            message.payload = strip_key(&message.payload, key);
        }
    }

    let put_back = crate::assemble::assemble(&recording).expect("an older file still opens");
    assert_eq!(put_back.records.len(), 4);
    assert_eq!(put_back.absent.capability, 4);
    assert_eq!(put_back.absent.nearest_person_m, 4);
    assert_eq!(put_back.absent.read_result, 4);
    assert!(!put_back.absent.verdict_inputs_all_recorded());

    // And the values are the ring's own "not known" ones rather than plausible defaults.
    let Body::Cycle(first) = &put_back.records[0].body else {
        panic!("a cycle")
    };
    assert_eq!(first.capability, NO_CAPABILITY);
    assert!(first.nearest_person_m.is_nan());
    assert_eq!(first.read, crate::record::ReadOutcome::Unrecorded);
}

/// Remove one top-level key from a canonical JSON object, the way an older writer's absence looks.
fn strip_key(payload: &str, key: &str) -> String {
    let needle = alloc::format!("\"{key}\":");
    let Some(start) = payload.find(&needle) else {
        return payload.to_string();
    };
    let bytes = payload.as_bytes();
    let mut depth = 0usize;
    let mut end = start + needle.len();
    while end < bytes.len() {
        match bytes[end] {
            b'[' | b'{' => depth += 1,
            b']' | b'}' if depth > 0 => depth -= 1,
            b'}' => break,
            b',' if depth == 0 => {
                end += 1;
                break;
            }
            _ => {}
        }
        end += 1;
    }
    let mut out = String::from(&payload[..start]);
    out.push_str(&payload[end..]);
    // A key removed from the end leaves the comma before it behind.
    out.replace(",}", "}")
}

#[test]
fn a_cycle_missing_one_of_its_four_messages_is_reported_rather_than_assembled() {
    // The failure a positional join has: a file truncated between two of a cycle's four messages
    // would otherwise assemble every cycle after it out of two different ones, and every field in
    // the result would look plausible.
    let (_, _, bytes) = window_with_a_capability_and_a_person(1.0);
    let mut recording = crate::read::read(&bytes).expect("a sealed window is readable");
    let before = recording.messages.len();
    recording
        .messages
        .retain(|m| !(m.channel_id == crate::schema::id::VERDICT && m.sequence == 2));
    assert_eq!(recording.messages.len(), before - 1);

    let put_back = crate::assemble::assemble(&recording).expect("the rest still assembles");
    assert_eq!(put_back.records.len(), 3, "one cycle is short a message");
    assert_eq!(put_back.incomplete.len(), 1);
    assert_eq!(put_back.incomplete[0].cycle, 2);
}

/// A cycle nobody posted a command on carries no verdict, and replaying one invents a comparison.
///
/// Found by replaying a real 300-cycle window off a bench: `skynetk` turned at 100 Hz while a
/// policy connected two seconds later, and the 167 cycles before it came back as 167 divergences
/// in which the watchdog tripped on a sequence that had never advanced. The loop had not asked the
/// kernel about any of them — *"with nothing posted there is nothing to evaluate and nothing to
/// send"* — so there was nothing in the recording to reproduce.
#[test]
fn a_cycle_the_kernel_was_never_asked_about_is_not_replayed() {
    let (mut ring, mut store) = ring_of(16);

    // Three idle cycles, exactly as `record()` writes one: no command has ever arrived, so the
    // sequence and stamp are zero, every channel is masked out, and `desired` is all `NaN`.
    for tick in 1..=3u64 {
        let owned = OwnedCycle {
            tick,
            desired: vec![f32::NAN; 3],
            command: vec![0.0; 3],
            mask: vec![0, 0, 0],
            clamped: vec![false; 3],
            position: vec![0.0; 3],
            velocity: vec![0.0; 3],
            effort: vec![0.0; 3],
            tripped: Vec::new(),
            response: None,
            read_result: 0,
        };
        let mut view = owned.view();
        view.command_stamp_ns = 0;
        view.command_seq = 0;
        ring.append(&mut store, tick * 1_000_000, Entry::Cycle(&view))
            .expect("a cycle");
    }

    let envelope = Envelope {
        joint_velocity_max_rad_s: Some(1.0),
        // The window that made the real failure visible: with one declared, an idle cycle's zero
        // stamp is stale by the whole uptime of the robot.
        watchdog_ms: Some(50.0),
        ..Envelope::default()
    };
    let clamp = clamp_at(1.0);
    let params = limits_params(envelope, &clamp);
    let limits = resolve_limits(&params);

    let result = replay_ring(&ring, &store, &limits, Some(&envelope));
    assert_eq!(result.cycles, 3);
    assert_eq!(result.unposted, 3, "none of them was posted");
    assert_eq!(result.evaluated(), 0);
    assert!(
        result.diverged.is_empty(),
        "a cycle the robot never evaluated cannot disagree with anything: {:?}",
        result.diverged
    );
    // And the window does not read as reproduced, because nothing in it was.
    assert!(
        !result.deterministic(),
        "vacuous truth: a window with nothing evaluated in it reproduced nothing"
    );
}

/// A held cycle **was** evaluated, and the emptiness of its `desired` is not what says otherwise.
///
/// The distinction cost a third of a real window. A policy publishing a hold masks every channel
/// out, so its cycles record an all-`NaN` `desired` exactly as an idle one does — and the kernel was
/// asked about those and did answer. Reading emptiness as absence collapsed a 300-cycle recording
/// to two evaluated cycles; §24's sequence is what separates them, because zero is a value a real
/// command can never carry.
#[test]
fn a_held_cycle_was_evaluated_even_though_it_commanded_nothing() {
    let (mut ring, mut store) = ring_of(16);
    let owned = OwnedCycle {
        tick: 1,
        desired: vec![f32::NAN; 3],
        command: vec![0.0; 3],
        mask: vec![0, 0, 0],
        clamped: vec![false; 3],
        position: vec![0.0; 3],
        velocity: vec![0.0; 3],
        effort: vec![0.0; 3],
        // What the kernel answered: a command arrived once and has not advanced since.
        tripped: vec![Guard::Watchdog],
        response: Some(FaultResponse::StopCat2),
        read_result: 0,
    };
    let mut view = owned.view();
    // A command *has* arrived — this is a hold, not an idle cycle.
    view.command_seq = 7;
    view.command_stamp_ns = 1_000;
    ring.append(&mut store, 900_000_000, Entry::Cycle(&view))
        .expect("a cycle");

    let envelope = Envelope {
        joint_velocity_max_rad_s: Some(1.0),
        watchdog_ms: Some(50.0),
        ..Envelope::default()
    };
    let clamp = clamp_at(1.0);
    let params = limits_params(envelope, &clamp);
    let limits = resolve_limits(&params);

    let result = replay_ring(&ring, &store, &limits, Some(&envelope));
    assert_eq!(result.unposted, 0, "a command had arrived");
    assert_eq!(result.evaluated(), 1);
    assert_eq!(
        result.agreed, 1,
        "the stale command trips the watchdog on replay exactly as it did on the robot: {:?}",
        result.diverged
    );
}

/// The predecessor follows §24's mask, because the loop refills it from the decision every cycle.
///
/// A held or idle cycle leaves the loop's `previous` **empty**, so the cycle after one is evaluated
/// with no predecessor and the rate limit has nothing to compare against. A replay that carried the
/// last command forward would hand that guard a comparison the robot never made.
#[test]
fn a_held_cycle_leaves_the_next_one_without_a_predecessor() {
    let (mut ring, mut store) = ring_of(16);
    for (tick, mask) in [
        (1u64, vec![1u8, 1, 1]),
        (2, vec![0, 0, 0]),
        (3, vec![1, 1, 1]),
    ] {
        let commanding = mask[0] != 0;
        let owned = OwnedCycle {
            tick,
            desired: if commanding {
                vec![0.4; 3]
            } else {
                vec![f32::NAN; 3]
            },
            command: vec![0.4; 3],
            mask,
            clamped: vec![false; 3],
            position: vec![0.0; 3],
            velocity: vec![0.0; 3],
            effort: vec![0.0; 3],
            tripped: Vec::new(),
            response: None,
            read_result: 0,
        };
        let mut view = owned.view();
        view.command_seq = tick as u32;
        view.command_stamp_ns = tick * 1_000_000;
        ring.append(&mut store, tick * 1_000_000, Entry::Cycle(&view))
            .expect("a cycle");
    }

    let envelope = Envelope {
        joint_velocity_max_rad_s: Some(1.0),
        ..Envelope::default()
    };
    let clamp = clamp_at(1.0);
    let params = limits_params(envelope, &clamp);
    let limits = resolve_limits(&params);

    let result = replay_ring(&ring, &store, &limits, Some(&envelope));
    assert_eq!(result.evaluated(), 3);
    // The second cycle inherits the first's command; the third inherits nothing, because the
    // second held every channel.
    assert_eq!(
        result.warmed, 1,
        "only the cycle after a commanding one has a predecessor"
    );
}
