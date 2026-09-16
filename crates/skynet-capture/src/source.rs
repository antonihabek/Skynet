//! Where frames come from, and the one loop that moves them (§47.2).
//!
//! [`Distributor`](crate::Distributor) answers *"which consumer sees which frame"* and deliberately
//! says nothing about where a frame came from. This module is the other half: the shape §47.2's
//! four backends share, a synthetic one that needs no hardware, and the loop that joins a source to
//! a distributor without leaking a buffer.
//!
//! # One trait, four backends
//!
//! §47.2 names V4L2, GenICam / GigE Vision, a vendor SDK, and the RHAL's own `read_sensor`. They
//! look nothing alike from the outside and are the same two operations underneath, because every
//! one of them owns a **fixed pool of buffers it lends out and wants back**:
//!
//! | §47.2 backend | [`Source::acquire`] | [`Source::release`] |
//! |---|---|---|
//! | V4L2 | `VIDIOC_DQBUF` | `VIDIOC_QBUF` |
//! | GenICam | `GetImage` | `QueueBuffer` |
//! | Vendor SDK | `wait_for_frames` | the frame's own destructor |
//! | RHAL `read_sensor` | one read into a driver-owned slot | the slot marked free |
//!
//! So the trait is those two and a declared depth, and nothing else. In particular there is no
//! `start`, no `stop` and no configuration: those differ across all four and none of them is on the
//! path a control loop runs, which is the only path this crate exists to keep honest.
//!
//! # Why the synthetic one is not a toy
//!
//! `DRY-COVERAGE-PLAN.md` commits to coverage with no robot in the room, and V4L2 cannot be
//! dry-verified at all — it needs a kernel device. What *can* be verified without one is every
//! claim §47.4 and §47.5 actually make, because **none of them involves a pixel**: which consumer
//! sees which frame, what happens when one falls behind, that a dropped frame is counted rather
//! than silently lost, and that a bounded pool stays bounded. [`Synthetic`] produces the handles
//! those claims are about, deterministically, so they are testable and replayable.
//!
//! It is also the honest shape of the thing it stands in for. A real camera that has lent out every
//! buffer produces nothing until one comes back; [`Synthetic`] does the same, by the same
//! mechanism, and that is the failure mode §47.5 says an unbounded pool hides.

use crate::{Captured, Distributor, StampSource};

/// A capture backend: a device that lends buffers out and wants them back (§47.2).
///
/// Implementations are **non-blocking**. `acquire` returning `None` means *nothing new since you
/// last asked*, never *wait*, because §47.4's invariant is that capture is never stalled and a loop
/// that can block here is one that can stall everything downstream of it.
pub trait Source {
    /// How many buffers this source owns.
    ///
    /// §47.5 requires the pool to be bounded and **declared per stream** rather than discovered, so
    /// that `skynet doctor` can show a number somebody chose. A source that cannot say has not
    /// declared one.
    fn depth(&self) -> usize;

    /// The next frame the device has finished acquiring, or `None`.
    ///
    /// The returned frame carries **one reference** to its buffer, owned by the caller.
    fn acquire(&mut self, now_ns: u64) -> Option<Captured>;

    /// Buffers currently lent out and not yet returned.
    ///
    /// Here so that *"the pool is dry"* can be **observed rather than probed**. The obvious way to
    /// ask is to call `acquire` and see whether it answers, and that is wrong in this trait: on a
    /// real device a dequeue that finds nothing is not free, and on any source that counts what it
    /// dropped it is not even side-effect-free. A question about state should not change it.
    fn outstanding(&self) -> usize;

    /// Take one more reference, because a second consumer now holds this buffer.
    ///
    /// §47.4 hands one frame to two consumers, so one buffer genuinely has two holders and a plain
    /// lend-and-return cannot say when it is free. This is what `dmabuf` does with an fd refcount,
    /// and what V4L2 with a GPU and an encoder on one buffer does underneath. The trait says it out
    /// loud so that a backend cannot get it wrong quietly.
    fn retain(&mut self, frame: Captured);

    /// Drop one reference. The buffer is the device's again at zero.
    ///
    /// Balanced against [`acquire`](Source::acquire) and [`retain`](Source::retain) exactly, or the
    /// pool either runs dry — capture stopping for a reason that looks like a driver fault — or
    /// hands a live buffer back to the device, which is §47.3's rule broken in the direction that
    /// corrupts a recording rather than the one that stops it.
    fn release(&mut self, frame: Captured);
}

/// What one turn of [`pump`] did.
///
/// Every field is a fact about *this* turn. The running totals live on the distributor's two
/// halves, which already count what they discarded; repeating them here would be a second record of
/// a number that can then disagree with the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Turn {
    /// Frames taken from the source and handed to both consumers.
    pub captured: usize,
    /// Frames the policy never read, overwritten by a fresher one.
    ///
    /// Not a fault. It is the slot doing exactly what §47.4 asks of it, and a control loop slower
    /// than its camera produces one of these per cycle by design.
    pub displaced: usize,
    /// Frames the recorder had no room for, and which the slot is therefore still holding.
    ///
    /// This one **is** worth an operator's attention, because §47.4 is explicit that the black box
    /// keeps completeness as its guarantee: each of these is a hole in the recording. It is
    /// annotated as a gap rather than left to look like a quiet period.
    pub not_recorded: usize,
    /// Whether the source had lent out every buffer it owns by the end of the turn.
    ///
    /// The bounded-pool failure §47.5 describes, observed rather than inferred: a source in this
    /// state produces nothing until a buffer comes back, and a loop that never releases sits here
    /// forever looking like a camera that stopped.
    pub source_exhausted: bool,
}

/// Move every frame the source has ready into the distributor, releasing as you go.
///
/// This is the whole of the capture loop, and it is short because the discipline is elsewhere: the
/// distributor decides who sees what, and the source owns the buffers. What this adds is the rule
/// that joins them — **whatever the distributor hands back is released in the same turn**. A frame
/// the policy never read and a frame the recorder refused are both buffers the device is waiting
/// for, and holding either until the next turn is how a pool drains one frame at a time.
///
/// `limit` bounds the work one turn may do, because a source that has fallen a long way behind
/// could otherwise hand back a hundred frames inside a control cycle. §47.4 says capture is never
/// stalled; it does not say a control loop may be.
pub fn pump<S: Source, const N: usize>(
    source: &mut S,
    into: &mut Distributor<N>,
    now_ns: u64,
    limit: usize,
) -> Turn {
    let mut turn = Turn::default();
    for _ in 0..limit {
        let Some(frame) = source.acquire(now_ns) else {
            break;
        };
        turn.captured += 1;
        let delivered = into.deliver(frame);

        // The reference `acquire` handed over **becomes the slot's**, because `publish` always
        // accepts: after `deliver` the slot holds this frame, every time, with no case where it
        // does not. So there is nothing to retain for the slot and nothing to release.
        //
        // The recorder is the consumer that may or may not have taken it, and a second holder is a
        // second reference.
        if delivered.not_recorded.is_none() {
            source.retain(frame);
        } else {
            // Counted, and **not released**. This frame is the one now sitting in the slot, so
            // releasing it here would hand a live buffer back to the device — see the note on
            // `Delivered::not_recorded`.
            turn.not_recorded += 1;
        }

        // What the slot gave up, on the other hand, is genuinely one holder fewer.
        if let Some(displaced) = delivered.displaced {
            turn.displaced += 1;
            source.release(displaced);
        }
    }
    turn.source_exhausted = source.depth() > 0 && source.outstanding() == source.depth();
    turn
}

/// A source with no hardware under it, for the coverage a dry build can actually have.
///
/// Deterministic in the strong sense: given the same construction and the same sequence of
/// `now_ns`, it produces the same frames with the same handles in the same order, on any machine.
/// That is what makes a capture test reproducible and what would let one be written as a corpus
/// case, neither of which is true of a camera.
///
/// `D` is the pool depth — §47.5's declared bound, as a const parameter so it cannot be exceeded by
/// construction rather than by a check somebody remembers to write.
#[derive(Debug, Clone)]
pub struct Synthetic<const D: usize> {
    stream: u16,
    period_ns: u64,
    next_ns: u64,
    seq: u64,
    stamp: StampSource,
    uncertainty_ns: u64,
    /// How many holders each buffer has. Index is the handle; zero is the device's again.
    refs: [u8; D],
    /// Frames the device finished but had no free buffer for. §47.4: dropped and counted.
    starved: u64,
}

impl<const D: usize> Synthetic<D> {
    /// A source whose device stamps its own frames, one every `period_ns` from `first_ns`.
    pub fn device(stream: u16, first_ns: u64, period_ns: u64) -> Synthetic<D> {
        Synthetic {
            stream,
            period_ns,
            next_ns: first_ns,
            seq: 0,
            stamp: StampSource::Device,
            uncertainty_ns: 0,
            refs: [0; D],
            starved: 0,
        }
    }

    /// A source that cannot stamp its own frames and says so, with the uncertainty §47.2 requires.
    ///
    /// The variant worth having, because it is the one a backend reaches for when it is about to do
    /// the forbidden thing. §47.2 permits an estimate and forbids quietly substituting arrival
    /// time, and the difference between the two is entirely whether a number was written down.
    pub fn estimated(
        stream: u16,
        first_ns: u64,
        period_ns: u64,
        uncertainty_ns: core::num::NonZeroU64,
    ) -> Synthetic<D> {
        Synthetic {
            stream,
            period_ns,
            next_ns: first_ns,
            seq: 0,
            stamp: StampSource::Estimated,
            uncertainty_ns: uncertainty_ns.get(),
            refs: [0; D],
            starved: 0,
        }
    }

    /// Frames the device produced with no buffer free to put them in.
    ///
    /// Counted rather than hidden, for §47.4's reason: a gap that is annotated is honest and a gap
    /// that looks like a quiet period is not. A camera does this too; it just does not tell you.
    pub fn starved(&self) -> u64 {
        self.starved
    }
}

impl<const D: usize> Source for Synthetic<D> {
    fn depth(&self) -> usize {
        D
    }

    fn outstanding(&self) -> usize {
        self.refs.iter().filter(|refs| **refs > 0).count()
    }

    fn acquire(&mut self, now_ns: u64) -> Option<Captured> {
        if D == 0 || now_ns < self.next_ns {
            return None;
        }
        let stamp_ns = self.next_ns;
        // The frame happened. Whether there is anywhere to put it decides only whether anybody
        // sees it, so the clock and the sequence advance either way — pretending the next frame is
        // this one would make a starved pool look like a slow camera, which is the substitution
        // §47.2 refuses one paragraph over about timestamps and refuses here for the same reason.
        //
        // The sequence advancing is what makes the loss visible downstream. A frame starved here
        // never reaches the record queue, so it is in none of its gaps: what a reader has instead
        // is a jump in `seq`, and `starved` is the count that says the jump was not their
        // imagination.
        self.next_ns = self.next_ns.saturating_add(self.period_ns);
        let seq = self.seq;
        self.seq += 1;
        // The lowest free buffer, which makes the handle sequence a property of the pool's state
        // rather than of an allocator — so a test can assert on it.
        let Some(handle) = self.refs.iter().position(|refs| *refs == 0) else {
            self.starved = self.starved.saturating_add(1);
            return None;
        };
        self.refs[handle] = 1;
        let handle = handle as u64;
        Some(match self.stamp {
            StampSource::Device => Captured::device(self.stream, seq, stamp_ns, handle),
            StampSource::Estimated => Captured::estimated(
                self.stream,
                seq,
                stamp_ns,
                core::num::NonZeroU64::new(self.uncertainty_ns).expect("estimated needs one"),
                handle,
            ),
        })
    }

    fn retain(&mut self, frame: Captured) {
        if let Some(refs) = self.refs.get_mut(frame.handle as usize) {
            *refs = refs.saturating_add(1);
        }
    }

    fn release(&mut self, frame: Captured) {
        if let Some(refs) = self.refs.get_mut(frame.handle as usize) {
            *refs = refs.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distributor whose recorder is smaller than the source's pool, so both failure modes are
    /// reachable in one fixture.
    fn fixture() -> (Synthetic<4>, Distributor<2>) {
        (Synthetic::device(7, 1_000, 1_000), Distributor::new())
    }

    /// The loop returns the buffers it took, which is §47.3's one hard rule.
    ///
    /// Stated as a pool-level invariant rather than a per-call one: after any number of turns,
    /// what the source has lent out is exactly what the distributor is still holding. A pump that
    /// leaked would drift apart from that within a few frames and never come back.
    #[test]
    fn every_buffer_the_pump_took_is_accounted_for() {
        let (mut source, mut into) = fixture();
        for turn in 1..=20u64 {
            pump(&mut source, &mut into, turn * 1_000, 8);
            // Distinct *buffers* held, not references: the slot and the recorder can be
            // holding the same one, which is the whole reason the source counts references.
            let mut handles = [false; 4];
            if let Some(frame) = into.latest.peek() {
                handles[frame.handle as usize] = true;
            }
            let mut parked = [None; 8];
            let mut at = 0;
            while let Some(frame) = into.record.pop() {
                handles[frame.handle as usize] = true;
                parked[at] = Some(frame);
                at += 1;
            }
            for frame in parked.into_iter().flatten() {
                assert_eq!(into.record.push(frame), crate::Push::Accepted);
            }
            let held = handles.iter().filter(|held| **held).count();
            assert_eq!(
                source.outstanding(),
                held,
                "turn {turn}: {} buffers lent out, {held} distinct held",
                source.outstanding()
            );
        }
    }

    /// §47.4's two consumers fail independently.
    ///
    /// The recorder here holds two frames and is never drained, so it refuses everything after
    /// them; the policy's slot takes every frame regardless. A recording with holes in it and a
    /// policy reading the freshest frame is the arrangement §47.4 asks for, and the failure it
    /// exists to prevent is the one where the encoder's stall reaches the policy.
    #[test]
    fn a_stalled_recorder_does_not_cost_the_policy_a_frame() {
        let (mut source, mut into) = fixture();
        let turn = pump(&mut source, &mut into, 4_000, 8);

        assert_eq!(turn.captured, 4, "four frames were due");
        assert_eq!(into.record.len(), 2, "the recorder took what it could hold");
        assert!(turn.not_recorded >= 1, "and refused the rest");
        assert!(into.latest.is_full(), "the policy still has the freshest");
        assert_eq!(
            into.latest.peek().map(|frame| frame.seq),
            Some(3),
            "and it is the freshest, not the first"
        );
        assert!(into.record.open_gap().is_some(), "the hole is annotated");
    }

    /// §47.5's bounded pool, at the bound.
    ///
    /// A source that has lent out every buffer produces nothing until one comes back. That is the
    /// real failure mode an unbounded pool hides, and the frames lost to it are counted rather
    /// than dropped silently — they are lost at the *device*, so they appear in none of the record
    /// queue's gaps and a reader has only the jump in `seq` to go on.
    #[test]
    fn a_pool_with_nothing_free_starves_and_says_so() {
        let mut source: Synthetic<2> = Synthetic::device(1, 0, 100);
        let mut into: Distributor<8> = Distributor::new();

        // Two frames fill the pool: the recorder keeps both and the slot holds the second, which
        // is the same buffer the recorder has. Nothing comes back, so nothing is free.
        pump(&mut source, &mut into, 100, 8);
        assert_eq!(source.outstanding(), 2, "both buffers are lent out");

        let turn = pump(&mut source, &mut into, 10_000, 8);
        assert_eq!(turn.captured, 0, "no buffer, no frame");
        assert!(
            turn.source_exhausted,
            "and the pump says which failure this is"
        );
        assert!(source.starved() > 0, "the lost frames are counted");

        // Drain the recorder and the frames flow again, which is the half that proves the stall
        // was the pool rather than the fixture.
        while let Some(frame) = into.record.pop() {
            source.release(frame);
        }
        let recovered = pump(&mut source, &mut into, 20_000, 8);
        assert!(recovered.captured > 0, "a returned buffer restarts capture");
    }

    /// §47.2: a source that cannot stamp its own frames says so, and names its uncertainty.
    #[test]
    fn an_estimated_source_never_passes_a_guess_off_as_a_measurement() {
        let uncertainty = core::num::NonZeroU64::new(250_000).unwrap();
        let mut source: Synthetic<2> = Synthetic::estimated(3, 0, 1_000, uncertainty);
        let frame = source.acquire(0).expect("a frame was due");
        assert_eq!(frame.stamp, StampSource::Estimated);
        assert_eq!(frame.uncertainty_ns, 250_000);
    }

    /// The same construction and the same clock produce the same frames, on any machine.
    ///
    /// The property the whole synthetic source exists for: a capture test that is reproducible,
    /// and therefore one that could be written as a corpus case rather than as a story about a
    /// camera somebody had.
    #[test]
    fn two_identical_sources_produce_identical_frames() {
        let run = || {
            let (mut source, mut into) = fixture();
            let mut seen = [(0u64, 0u64); 12];
            let mut at = 0;
            for turn in 1..=6u64 {
                pump(&mut source, &mut into, turn * 1_000, 8);
                while let Some(frame) = into.record.pop() {
                    seen[at] = (frame.seq, frame.handle);
                    at += 1;
                    source.release(frame);
                }
            }
            (seen, at, source.starved())
        };
        assert_eq!(run(), run());
    }

    /// A source with no buffers at all is refused by answering nothing, not by dividing by zero.
    #[test]
    fn a_pool_of_none_captures_nothing() {
        let mut source: Synthetic<0> = Synthetic::device(0, 0, 10);
        let mut into: Distributor<4> = Distributor::new();
        let turn = pump(&mut source, &mut into, 1_000, 8);
        assert_eq!(turn.captured, 0);
        assert!(
            !turn.source_exhausted,
            "a pool of none is not a pool that ran dry"
        );
    }

    /// The trap the first backend would have fallen into, pinned.
    ///
    /// `Delivered::not_recorded` is the frame that was *just published*, handed back only to say
    /// the recorder refused it — and `publish` always accepts, so the slot holds that buffer as
    /// `deliver` returns. A pump that released it would hand a live buffer to the device, and the
    /// symptom is a policy reading a frame something else is writing, three hours in.
    ///
    /// Asserted as the property rather than the line: after a turn in which the recorder refused
    /// everything, the buffer the slot holds is still lent out.
    #[test]
    fn a_frame_the_recorder_refused_is_not_released_because_the_slot_has_it() {
        let mut source: Synthetic<4> = Synthetic::device(0, 0, 100);
        let mut into: Distributor<0> = Distributor::new();

        let turn = pump(&mut source, &mut into, 100, 2);
        assert!(turn.not_recorded > 0, "a queue of zero records nothing");

        let held = into.latest.peek().expect("the slot took it anyway").handle;
        assert_eq!(
            source.outstanding(),
            1,
            "exactly the buffer the slot is holding is still lent out"
        );

        // And it becomes the device's again only once the policy is done with it.
        let taken = into.latest.take().expect("the policy reads it");
        assert_eq!(taken.handle, held);
        source.release(taken);
        assert_eq!(source.outstanding(), 0, "released once, and once is right");
    }
}
