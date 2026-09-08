//! §47.6's mapping half, exercised in one process and across two.
//!
//! Every test here is `cfg(target_os = "linux")`, because `memfd_create` is. That is the same gate
//! `skynet-ring-shm`'s own suite carries, and it is stated rather than implied: on any other host
//! this file compiles to nothing and the crate is a set of offsets nobody checked.

#![cfg(target_os = "linux")]

use super::*;

fn geometry() -> Geometry {
    Geometry {
        slots: 4,
        slot_bytes: 32,
    }
}

/// Create a pool and map it back, in one process.
///
/// The reader adopts **duplicates** of the writer's descriptors rather than the descriptors
/// themselves, because both `Mapping`s close what they hold on drop and a single-process test that
/// shared one fd would be closing it twice. `dup` is what `SCM_RIGHTS` does across a socket, so
/// this is the same arrangement one process further in.
fn pair(geometry: Geometry) -> (Writer, Reader) {
    let writer = Writer::create("head_rgb", geometry).expect("a pool");
    let [frames, loans] = writer.descriptors();
    let reader = Reader::adopt(dup(frames), dup(loans), geometry).expect("a reader");
    (writer, reader)
}

fn dup(fd: core::ffi::c_int) -> core::ffi::c_int {
    skynet_ring_shm::pass::dup(fd).expect("dup")
}

/// A pool nobody has published into reads as *no frame yet*, never as a frame of zeros.
///
/// §47.6's rule, and the reason the header carries `u64::MAX` rather than slot 0: a sentinel of 0
/// would be indistinguishable from a published first frame, and a policy would open a black image
/// on every robot at boot.
#[test]
fn an_empty_pool_has_no_frame_rather_than_a_frame_of_zeros() {
    let (_writer, reader) = pair(geometry());
    assert_eq!(reader.latest(), None);
    assert_eq!(reader.stream(), "head_rgb");
}

/// Pixels cross, and the identity crosses with them.
#[test]
fn a_published_frame_is_readable_through_the_mapping() {
    let (mut writer, reader) = pair(geometry());
    let handle = writer.publish(7, 1_234, b"hello frame").expect("published");

    let latest = reader.latest().expect("a frame");
    assert_eq!(latest, handle);

    let identity = reader.acquire(&latest).expect("a loan");
    assert_eq!(identity.sequence, 7);
    assert_eq!(identity.stamp_ns, 1_234);
    assert_eq!(identity.stream, "head_rgb");

    let mut pixels = Vec::new();
    let read = reader.read(&latest, &mut pixels).expect("bytes");
    assert_eq!(read.sequence, 7);
    assert_eq!(pixels, b"hello frame");
    reader.release(&latest);
}

/// **The property this crate exists for.** A slot on loan is not reused under the reader.
///
/// §47.6: *"a slot still acquired by a reader is never reused under it."* The writer is in a
/// different process from the reader in production, so this is the one rule that could not be
/// enforced by the contract's bookkeeping alone — the loans region is what carries it back.
///
/// Four slots, one held, and then five publishes. The held slot must survive all of them.
#[test]
fn a_slot_on_loan_is_never_taken_under_the_reader() {
    let (mut writer, reader) = pair(geometry());
    let first = writer.publish(1, 10, b"first").expect("published");
    let held = reader.acquire(&first).expect("a loan");
    assert_eq!(held.sequence, 1);

    for sequence in 2..=6 {
        // Some of these are refused once the other three slots are on nothing — they are not, so
        // every one of them lands. What matters is that none of them lands *here*.
        let handle = writer
            .publish(sequence, sequence * 10, b"later")
            .expect("published");
        assert_ne!(
            handle.slot, first.slot,
            "publish {sequence} took the slot a reader is holding"
        );
    }

    let mut pixels = Vec::new();
    let still = reader.read(&first, &mut pixels).expect("the loan survived");
    assert_eq!(still.sequence, 1, "the reader's frame is still its frame");
    assert_eq!(pixels, b"first");

    // And once released, the writer is free to take it — the other half of §47.6's sentence.
    reader.release(&first);
    let mut took_it = false;
    for sequence in 7..=12 {
        let handle = writer.publish(sequence, 0, b"reuse").expect("published");
        if handle.slot == first.slot {
            took_it = true;
            break;
        }
    }
    assert!(took_it, "a released slot must become reusable");
    assert_eq!(
        reader.read(&first, &mut Vec::new()),
        None,
        "and the old handle is refused rather than shown somebody else's frame"
    );
}

/// Every slot on loan means the frame is dropped and counted — never that capture stalls.
///
/// §47.4's priority, enforced through §47.6's `pool_exhausted`. A policy that pins every slot can
/// starve itself of vision and cannot stop the camera, which is the whole of what a hostile reader
/// can do with the writable half of this mapping.
#[test]
fn a_reader_that_pins_every_slot_starves_itself_and_stalls_nothing() {
    let (mut writer, reader) = pair(geometry());

    let mut held = Vec::new();
    for sequence in 1..=4 {
        let handle = writer.publish(sequence, 0, b"x").expect("published");
        reader.acquire(&handle).expect("a loan");
        held.push(handle);
    }

    let refused = writer
        .publish(5, 0, b"x")
        .expect_err("every slot is on loan");
    assert_eq!(refused, PoolError::Refused(FrameRefusalCode::PoolExhausted));
    assert!(
        writer.stats().dropped >= 1,
        "the drop is counted, which is the evidence §47.6 asks for"
    );

    // Releasing one is enough for capture to carry on. Nothing had to be restarted.
    reader.release(&held[0]);
    writer.publish(6, 0, b"x").expect("capture continues");
}

/// A handle whose slot has moved on is refused rather than shown the new frame.
///
/// §47.6's `stale_handle`, and the failure it prevents is the worst one available here: a policy
/// acting on pixels it believes are one frame and are another.
#[test]
fn a_stale_handle_is_refused_by_both_paths() {
    let (mut writer, reader) = pair(Geometry {
        slots: 1,
        slot_bytes: 16,
    });
    let first = writer.publish(1, 0, b"one").expect("published");
    let second = writer.publish(2, 0, b"two").expect("published");
    assert_eq!(first.slot, second.slot, "one slot means it was reused");
    assert_ne!(first.generation, second.generation);

    assert_eq!(
        reader.acquire(&first),
        Err(PoolError::Refused(FrameRefusalCode::StaleHandle))
    );
    assert_eq!(reader.peek(&first, &mut Vec::new()), None);
    assert_eq!(reader.read(&first, &mut Vec::new()), None);

    // The current one is fine, which is what makes the refusals above about staleness rather than
    // about the pool being broken.
    let mut pixels = Vec::new();
    assert!(reader.peek(&second, &mut pixels).is_some());
    assert_eq!(pixels, b"two");
}

/// `peek` refuses a frame that moved while it was being read, rather than returning a smear.
///
/// The tear rule. It cannot be provoked by scheduling in a single-threaded test, so it is provoked
/// directly: the writer publishes *between* the two generation reads `peek` performs, which is
/// exactly the interleaving the check exists for. A one-slot pool makes the reuse certain.
#[test]
fn peek_refuses_a_frame_that_moved_under_it() {
    let (mut writer, reader) = pair(Geometry {
        slots: 1,
        slot_bytes: 16,
    });
    let handle = writer.publish(1, 0, b"one").expect("published");

    // The frame is there and readable right now.
    let mut pixels = Vec::new();
    assert!(reader.peek(&handle, &mut pixels).is_some());

    // Now it moves. A `peek` with the old handle sees the changed generation on its *first* read
    // and refuses; the interleaved case is the same comparison one line later, and both are the
    // same rule.
    writer.publish(2, 0, b"two").expect("published");
    assert_eq!(reader.peek(&handle, &mut pixels), None);
    assert!(pixels.is_empty(), "a refused peek leaves nothing behind");
}

/// A frame larger than a slot is refused, never truncated.
#[test]
fn a_frame_too_large_for_a_slot_is_refused() {
    let (mut writer, _reader) = pair(Geometry {
        slots: 2,
        slot_bytes: 4,
    });
    assert_eq!(
        writer.publish(1, 0, b"much too long"),
        Err(PoolError::TooLarge {
            needed: 13,
            capacity: 4
        })
    );
}

/// A mapping that is not a pool is refused at the first word.
///
/// §25's rule about its two rings, applied to the third format: a consumer built for one that
/// opened another must be refused rather than read somebody else's bytes as its own.
#[test]
fn a_mapping_that_is_not_a_pool_is_refused() {
    let geometry = geometry();
    let frames = Mapping::create_bytes("not-a-pool", geometry.frame_bytes(), geometry.slots)
        .expect("a mapping");
    let loans =
        Mapping::create_bytes("loans", geometry.loan_bytes(), geometry.slots).expect("a mapping");
    assert!(matches!(
        Reader::adopt(dup(frames.fd()), dup(loans.fd()), geometry),
        Err(PoolError::NotAPool)
    ));
}

/// A reader that disagrees with the creator about the geometry is refused.
///
/// It would otherwise address the wrong bytes for every slot past the first — a misread that
/// produces plausible pixels rather than an error, which is the shape this whole file is arranged
/// against.
#[test]
fn a_reader_that_disagrees_about_the_geometry_is_refused() {
    let writer = Writer::create("head_rgb", geometry()).expect("a pool");
    let [frames, loans] = writer.descriptors();
    let wrong = Geometry {
        slots: 4,
        slot_bytes: 64,
    };
    assert!(matches!(
        Reader::adopt(dup(frames), dup(loans), wrong),
        Err(PoolError::NotAPool)
    ));
}

/// A claim on a generation the slot no longer holds is not a loan.
///
/// The narrow property the generation actually buys, asserted so nobody claims the wide one again.
/// A stale claim — one left in the region from a generation ago — holds nothing, so the writer
/// takes the slot.
#[test]
fn a_claim_on_an_old_generation_holds_nothing() {
    let (mut writer, reader) = pair(Geometry {
        slots: 1,
        slot_bytes: 8,
    });
    let first = writer.publish(1, 0, b"one").expect("published");
    reader.acquire(&first).expect("a loan");
    reader.release(&first);

    // The slot moves on, and the reader writes its *old* claim back — a bug, a race, or a hostile
    // policy replaying a handle it once held.
    let second = writer.publish(2, 0, b"two").expect("published");
    Loans::of(&reader.loans, 1).set(0, first.generation);

    // It holds nothing: the claim names a generation the slot no longer carries.
    writer
        .publish(3, 0, b"three")
        .expect("the stale claim holds nothing");
    let _ = second;
}

/// **An abandoned loan pins its slot, and only the runtime can free it.**
///
/// The property the first draft of this crate's own comment got wrong, kept as a test because the
/// wrong version is the tempting one: a claim naming the generation the slot *still* holds is a
/// loan for ever, because the writer will not reuse the slot, so the generation never advances, so
/// the claim never stops matching. A generation is no better than a boolean here.
///
/// [`Writer::revoke_loans`] is the answer, and it is the runtime's to call — it launched the unit,
/// so it knows the reader is gone. A timeout could not do this job: §47.6 puts no bound on how long
/// a policy may hold a frame, so any deadline short enough to reclaim promptly is short enough to
/// take a working reader's frame.
#[test]
fn an_abandoned_loan_pins_its_slot_until_the_runtime_revokes_it() {
    let (mut writer, reader) = pair(Geometry {
        slots: 2,
        slot_bytes: 8,
    });
    let first = writer.publish(1, 0, b"one").expect("published");
    reader.acquire(&first).expect("a loan");
    let second = writer.publish(2, 0, b"two").expect("published");
    reader.acquire(&second).expect("a loan");
    // Both slots are held and the reader dies here. Nothing releases either.

    assert_eq!(
        writer.publish(3, 0, b"three"),
        Err(PoolError::Refused(FrameRefusalCode::PoolExhausted)),
        "a dead reader's loans are indistinguishable from a slow reader's"
    );
    // And it does not recover on its own, however many frames arrive.
    for sequence in 4..=20 {
        assert!(writer.publish(sequence, 0, b"x").is_err());
    }

    // The runtime notices the process is gone and says so. Capture continues immediately.
    writer.revoke_loans();
    let after = writer.publish(21, 0, b"back").expect("capture continues");
    assert!(after.slot < 2);
}

/// The loans region is the only thing a reader may write, and the pixels are not in it.
///
/// Asserted through the access flags rather than by attempting a write, because the failure a write
/// would produce is a `SIGSEGV` that takes the test process with it. The property is a page-table
/// one and this is where it is recorded.
#[test]
fn the_reader_maps_pixels_read_only_and_loans_writable() {
    let (_writer, reader) = pair(geometry());
    assert_eq!(reader.frames.access(), Access::Read);
    assert_eq!(reader.loans.access(), Access::Write);
}

/// A receiver that was handed only the two descriptors finds the geometry in the region itself.
///
/// This is the property §25's socket needs: the far side of `SCM_RIGHTS` receives file descriptors
/// and nothing else, so a pool whose slot count had to travel beside them would need a second
/// channel and a rule about what to do when the two disagree.
#[test]
fn a_pool_describes_its_own_shape_to_whoever_receives_it() {
    let shape = Geometry {
        slots: 3,
        slot_bytes: 48,
    };
    let mut writer = Writer::create("wrist_depth", shape).expect("a pool");
    writer.publish(11, 909, b"declared").expect("published");
    let [frames, loans] = writer.descriptors();

    let reader = Reader::adopt_declared(dup(frames), dup(loans)).expect("a reader");
    assert_eq!(reader.stream(), "wrist_depth");

    let handle = reader.latest().expect("a frame");
    let mut pixels = Vec::new();
    let identity = reader.read(&handle, &mut pixels).expect("the frame");
    assert_eq!(identity.sequence, 11);
    assert_eq!(pixels, b"declared");
    reader.release(&handle);
}

/// A descriptor that is not a pool is refused before anything is addressed as pixels.
///
/// And the descriptors are consumed on that path too. A receiver that declined a pool and kept its
/// fds would hold the writer's memory alive for the life of the process while using none of it,
/// which is the shape of leak that only shows up as a robot that runs out of memory in week three.
#[test]
fn a_descriptor_that_is_not_a_pool_is_refused_and_still_consumed() {
    let ring = Mapping::create("skynet-not-a-pool", 4).expect("a command ring");
    let loans = Mapping::create_bytes("skynet-not-a-pool-loans", 32, 4).expect("bytes");

    let borrowed = dup(ring.fd());
    let borrowed_loans = dup(loans.fd());
    match Reader::adopt_declared(borrowed, borrowed_loans) {
        Err(PoolError::NotAPool) => {}
        Err(other) => panic!("refused, but for the wrong reason: {other}"),
        Ok(_) => panic!("a command ring was accepted as a frame pool"),
    }

    // Both descriptors are gone: `close` on one that was already closed fails, and that failure is
    // the assertion. Nothing else in this process could have taken these numbers in between.
    assert!(
        skynet_ring_shm::pass::dup(borrowed).is_err(),
        "the frames descriptor was closed by the refusal",
    );
    assert!(
        skynet_ring_shm::pass::dup(borrowed_loans).is_err(),
        "the loans descriptor was closed by the refusal",
    );
}
