//! §47.4's claim, exercised.
//!
//! The section's argument is that a single queue serving both consumers *"fails in the direction
//! nobody wants"*, and every test here is about a direction rather than about an API. What has to
//! hold is that the policy stays fresh while the encoder stalls, that the encoder's loss is
//! recorded rather than absorbed, and that neither of those ever reaches back into capture.

use super::*;

fn frame(stream: u16, seq: u64) -> Captured {
    // Stamps a millisecond apart, so a gap's `from_ns`/`to_ns` are distinguishable and a test that
    // got them the wrong way round fails rather than passing on two equal numbers.
    Captured::device(stream, seq, seq * 1_000_000, 0x1000 + seq)
}

// ── The policy's side ────────────────────────────────────────────────────────

#[test]
fn the_slot_hands_back_the_frame_it_overwrote() {
    // The zero-copy rule: a buffer handed onward is handed back, or the pool runs dry and capture
    // stops for a reason that looks like a driver fault.
    let mut slot = LatestSlot::new();
    assert_eq!(slot.publish(frame(0, 1)), None);
    assert_eq!(slot.publish(frame(0, 2)), Some(frame(0, 1)));
    assert_eq!(slot.take(), Some(frame(0, 2)));
    assert_eq!(slot.take(), None);
}

#[test]
fn a_slow_policy_reads_the_newest_frame_and_not_a_queue_of_stale_ones() {
    // The sentence this crate exists for: "a queue in front of a policy is latency wearing the
    // costume of reliability". Ten frames arrive during one long cycle; the policy gets the tenth.
    let mut slot = LatestSlot::new();
    for seq in 1..=10 {
        slot.publish(frame(0, seq));
    }
    assert_eq!(slot.take(), Some(frame(0, 10)));
    assert_eq!(slot.overwritten(), 9);
}

#[test]
fn the_overwrite_count_is_not_a_fault_count() {
    // A steady count means capture is faster than the policy, which is the intended arrangement:
    // being on the freshest frame *is* discarding the ones before it. Asserted so nobody later
    // wires this counter to an alarm.
    let mut slot = LatestSlot::new();
    slot.publish(frame(0, 1));
    slot.take();
    slot.publish(frame(0, 2));
    slot.take();
    assert_eq!(
        slot.overwritten(),
        0,
        "a frame that was read was not overwritten"
    );
}

#[test]
fn taking_empties_the_slot_so_one_buffer_reaches_one_owner() {
    // A policy that read the same buffer twice would run a cycle on a frame it had already acted
    // on, and the second read would also be a second owner of one handle.
    let mut slot = LatestSlot::new();
    slot.publish(frame(0, 1));
    assert!(slot.is_full());
    assert_eq!(slot.take(), Some(frame(0, 1)));
    assert!(!slot.is_full());
    assert_eq!(slot.peek(), None);
}

// ── The black box's side ─────────────────────────────────────────────────────

#[test]
fn the_queue_is_oldest_first_and_does_not_rewrite_its_own_past() {
    // Unlike the slot. The recording's guarantee is completeness and order, and a queue that
    // dropped its oldest frame to make room for a newer one would rewrite its past under load.
    let mut queue: RecordQueue<3> = RecordQueue::new();
    for seq in 1..=3 {
        assert_eq!(queue.push(frame(0, seq)), Push::Accepted);
    }
    assert_eq!(queue.push(frame(0, 4)), Push::Dropped(frame(0, 4)));
    assert_eq!(queue.pop(), Some(frame(0, 1)));
    assert_eq!(queue.pop(), Some(frame(0, 2)));
    assert_eq!(queue.pop(), Some(frame(0, 3)));
    assert_eq!(queue.pop(), None);
}

#[test]
fn a_full_queue_hands_the_frame_back_rather_than_swallowing_it() {
    let mut queue: RecordQueue<1> = RecordQueue::new();
    queue.push(frame(0, 1));
    match queue.push(frame(0, 2)) {
        Push::Dropped(returned) => assert_eq!(returned, frame(0, 2)),
        Push::Accepted => panic!("a full queue accepted a frame"),
    }
}

#[test]
fn consecutive_drops_are_one_gap_and_not_a_hundred() {
    // A hundred gaps of one frame each is the same information rendered unreadable.
    let mut queue: RecordQueue<1> = RecordQueue::new();
    queue.push(frame(0, 1));
    for seq in 2..=101 {
        queue.push(frame(0, seq));
    }
    let gap = queue.open_gap().copied().expect("a gap is open");
    assert_eq!(gap.frames, 100);
    assert_eq!(gap.from_seq, 2);
    assert_eq!(gap.to_seq, 101);
    assert_eq!(gap.from_ns, 2_000_000);
    assert_eq!(gap.to_ns, 101_000_000);
    assert_eq!(queue.dropped(), 100);
}

#[test]
fn a_gap_is_closed_once_and_counted_once() {
    let mut queue: RecordQueue<1> = RecordQueue::new();
    queue.push(frame(0, 1));
    queue.push(frame(0, 2));

    let gap = queue.close_gap().expect("the gap");
    assert_eq!(gap.frames, 1);
    assert_eq!(queue.gaps(), 1);
    assert!(
        queue.close_gap().is_none(),
        "closing twice invents a second gap"
    );
    assert_eq!(queue.gaps(), 1);
}

#[test]
fn two_hundred_frames_in_one_gap_and_in_two_hundred_gaps_are_told_apart() {
    // The reason `dropped` and `gaps` are separate counters: one is a stall and the other is a
    // queue sized one frame too small, and they call for completely different work.
    let mut stall: RecordQueue<1> = RecordQueue::new();
    stall.push(frame(0, 0));
    for seq in 1..=200 {
        stall.push(frame(0, seq));
    }
    stall.close_gap();
    assert_eq!((stall.dropped(), stall.gaps()), (200, 1));

    // The other shape: full, drop one, drain, repeat. Every drop is its own gap.
    let mut tight: RecordQueue<1> = RecordQueue::new();
    for seq in 0..200 {
        tight.push(frame(0, seq * 2));
        tight.push(frame(0, seq * 2 + 1));
        tight.close_gap();
        tight.pop();
    }
    assert_eq!((tight.dropped(), tight.gaps()), (200, 200));
}

#[test]
fn a_gap_belongs_to_one_stream() {
    // Two streams sharing a gap record would report a loss on a camera that never lost a frame.
    let mut queue: RecordQueue<1> = RecordQueue::new();
    queue.push(frame(0, 1));
    queue.push(frame(0, 2));
    queue.push(frame(7, 5));
    let gap = queue.open_gap().copied().expect("a gap");
    assert_eq!(
        gap.stream, 7,
        "a frame from another stream starts its own gap"
    );
    assert_eq!(gap.frames, 1);
}

#[test]
fn a_queue_of_zero_records_nothing_and_says_so() {
    // "Do not record this stream" is a real configuration — a robot with no storage for a 1080p
    // stream — and the honest form of it is an account of what was not kept, not a panic in a
    // capture path and not silence.
    let mut queue: RecordQueue<0> = RecordQueue::new();
    assert_eq!(queue.capacity(), 0);
    assert_eq!(queue.push(frame(0, 1)), Push::Dropped(frame(0, 1)));
    assert_eq!(queue.pop(), None);
    assert_eq!(queue.dropped(), 1);
    assert_eq!(queue.close_gap().map(|gap| gap.frames), Some(1));
}

#[test]
fn the_queue_wraps_without_losing_order() {
    let mut queue: RecordQueue<3> = RecordQueue::new();
    for seq in 1..=3 {
        queue.push(frame(0, seq));
    }
    assert_eq!(queue.pop(), Some(frame(0, 1)));
    assert_eq!(queue.push(frame(0, 4)), Push::Accepted);
    assert_eq!(queue.pop(), Some(frame(0, 2)));
    assert_eq!(queue.pop(), Some(frame(0, 3)));
    assert_eq!(queue.pop(), Some(frame(0, 4)));
    assert!(queue.is_empty());
}

// ── The two together, which is the decision ──────────────────────────────────

#[test]
fn a_stalled_encoder_does_not_add_latency_to_the_policy() {
    // §47.4's claim, stated as the test it deserves. The encoder stops; sixty frames arrive; the
    // policy still reads the sixtieth, and the recording — not the control path — is what lost.
    let mut dist: Distributor<4> = Distributor::new();
    let mut leaked = 0u64;

    for seq in 1..=60 {
        let delivered = dist.deliver(frame(0, seq));
        // Every handle that left the pipeline is accounted for, which is what the release rule
        // means in practice.
        if delivered.displaced.is_some() {
            leaked += 1;
        }
        if delivered.not_recorded.is_some() {
            leaked += 1;
        }
    }

    assert_eq!(
        dist.latest.take(),
        Some(frame(0, 60)),
        "the policy reads the newest"
    );
    assert_eq!(dist.record.len(), 4, "the recording is bounded");
    assert_eq!(dist.record.dropped(), 56);
    assert_eq!(
        leaked,
        59 + 56,
        "59 overwritten, 56 not recorded, all returned"
    );
}

#[test]
fn the_two_consumers_fail_independently() {
    // The property a single queue destroys. A frame the recorder refused still reaches the policy,
    // and a frame the policy never read is still recorded.
    let mut dist: Distributor<1> = Distributor::new();

    dist.deliver(frame(0, 1));
    let second = dist.deliver(frame(0, 2));
    assert!(second.not_recorded.is_some(), "the recorder is full");
    assert_eq!(
        dist.latest.peek(),
        Some(&frame(0, 2)),
        "the policy still got it"
    );

    // And the other way. Drain the recorder, publish two more, read neither: both are recorded.
    dist.record.pop();
    dist.record.close_gap();
    dist.latest.take();
    dist.deliver(frame(0, 3));
    assert_eq!(dist.record.len(), 1);
    assert_eq!(dist.record.pop(), Some(frame(0, 3)));
}

#[test]
fn delivery_never_reports_a_failure_because_capture_is_never_stalled() {
    // `deliver` returns what to release, not whether it worked. There is no error path, and that
    // absence is the invariant: stalling capture to protect the recording starves the policy, which
    // is precisely backwards.
    let mut dist: Distributor<0> = Distributor::new();
    for seq in 1..=1_000 {
        let delivered = dist.deliver(frame(0, seq));
        assert!(
            delivered.not_recorded.is_some(),
            "nothing is recorded at capacity zero"
        );
    }
    assert_eq!(dist.latest.take(), Some(frame(0, 1_000)));
    assert_eq!(dist.record.dropped(), 1_000);
}

// ── Timestamps ───────────────────────────────────────────────────────────────

#[test]
fn a_device_stamp_carries_no_uncertainty_and_an_estimate_must() {
    // §47.2: estimating is acceptable, silently substituting arrival time is not. The type makes
    // the second unspellable — there is no way to construct an estimate with zero uncertainty.
    let measured = Captured::device(0, 1, 1_000, 7);
    assert_eq!(measured.stamp, StampSource::Device);
    assert_eq!(measured.uncertainty_ns, 0);

    let guessed = Captured::estimated(
        0,
        1,
        1_000,
        core::num::NonZeroU64::new(20_000_000).expect("non-zero"),
        7,
    );
    assert_eq!(guessed.stamp, StampSource::Estimated);
    assert_eq!(guessed.uncertainty_ns, 20_000_000);
}

#[test]
fn the_sequence_survives_a_drop_so_the_black_box_can_see_the_hole_itself() {
    // The gap record is the annotation; the sequence is the evidence underneath it. A reader with
    // the recording and no counters can still tell that frames 2 to 4 are missing.
    let mut queue: RecordQueue<2> = RecordQueue::new();
    queue.push(frame(0, 1));
    queue.push(frame(0, 2));
    queue.push(frame(0, 3));
    queue.push(frame(0, 4));
    queue.pop();
    queue.pop();
    queue.close_gap();
    queue.push(frame(0, 5));

    assert_eq!(queue.pop().map(|f| f.seq), Some(5));
    assert_eq!(queue.dropped(), 2);
}
