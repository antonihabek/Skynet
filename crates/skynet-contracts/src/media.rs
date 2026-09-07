//! Frame handles for the sensor and media pipeline (§47.6).
//!
//! The Rust edition of `reference/typescript/media.ts`. Read that file for the design; this one
//! records the port and the two places the port had a choice to make.
//!
//! §47.4 gives the policy a latest-wins slot and the black box a bounded queue, and says nothing
//! about how a frame reaches a policy **in another process**. §80 rules out the obvious answer:
//! §25's ring has 4 KiB slots sized for joint vectors, a frame does not fit in one, and widening
//! the ring to hold images would put a megabyte of copy in the path a control loop runs. So frames
//! stay where capture put them, and what crosses is a *handle* — `(stream, slot, generation)`,
//! enough to map exactly one frame read-only and not enough to name a different one.
//!
//! # What this is not
//!
//! [`skynet_capture`](https://docs.rs/skynet-capture) is §47.4: two consumers inside the process
//! that holds the camera, one discarding to stay fresh and one retaining to stay complete. This is
//! §47.6: the **loan**, and the rules that make a handle safe to hand across a sandbox boundary
//! where the holder is code the OS did not write and cannot trust to give a buffer back promptly.
//! They are adjacent and they are not the same object. The pool decides which slot a frame lands
//! in and refuses a handle that has outlived its frame; the split decides which of two consumers
//! sees which frame.
//!
//! # The bookkeeping half
//!
//! Every function here is arithmetic over slot state. The **mapping** half — a read-only mapping
//! of the named slot in the policy's address space — is per-OS code that cannot be expressed in a
//! contract, and what it must honour is the one property this module exists to guarantee: a handle
//! grants read access to exactly one generation of one slot, and the pool may reuse that slot the
//! moment it is released.
//!
//! # Two notes on the port
//!
//! **`Vec<Slot>` rather than a const-generic array.** Capacity is declared per stream (§47.5) and
//! read from the descriptor, so it is a runtime number here as it is in the reference. This crate
//! is `no_std` with `alloc`, and the allocation happens once when a stream is opened rather than
//! per frame; nothing in `publish`, `acquire` or `release` allocates. An alloc-free port for a
//! control loop that cannot afford even that would be held to the same corpus, which is what makes
//! a second implementation of this safe to have if one is ever wanted.
//!
//! **The refusals carry their message.** Every other edition renders the sentence, so this one
//! does too — §64 makes an error a design surface, and a Rust edition that returned a bare code
//! while the reference returned a sentence would diverge in the corpus on the field an operator
//! actually reads.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

/// Everything a frame pool can refuse, in the order a reader meets them.
///
/// The last member is the odd one and belongs in the set anyway: [`FrameRefusalCode::PoolExhausted`]
/// refuses *capture*, not the reader, and it is the only one of the six that is a normal operating
/// condition rather than a defect somewhere.
pub const FRAME_REFUSAL_CODES: [&str; 6] = [
    "unknown_stream",
    "unknown_slot",
    "stale_handle",
    "already_acquired",
    "double_release",
    "pool_exhausted",
];

/// Why a pool refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRefusalCode {
    /// A frame from a stream this pool does not hold.
    UnknownStream,
    /// A handle naming a slot outside this pool, or another pool's stream.
    UnknownSlot,
    /// The slot has been reused since the handle was issued.
    StaleHandle,
    /// The handle is already checked out; a handle is one loan, not a share.
    AlreadyAcquired,
    /// The handle was already released.
    DoubleRelease,
    /// Every slot is acquired, so capture's frame was dropped and counted.
    PoolExhausted,
}

impl FrameRefusalCode {
    /// The wire spelling, which is the corpus's.
    pub fn as_str(self) -> &'static str {
        match self {
            FrameRefusalCode::UnknownStream => "unknown_stream",
            FrameRefusalCode::UnknownSlot => "unknown_slot",
            FrameRefusalCode::StaleHandle => "stale_handle",
            FrameRefusalCode::AlreadyAcquired => "already_acquired",
            FrameRefusalCode::DoubleRelease => "double_release",
            FrameRefusalCode::PoolExhausted => "pool_exhausted",
        }
    }
}

/// A refusal, with the sentence §64 asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRefusal {
    /// Which refusal.
    pub code: FrameRefusalCode,
    /// What a reader is told, in English.
    pub message: String,
}

impl FrameRefusal {
    /// `{"code": …, "message": …}`.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("code", Json::Str(self.code.as_str().to_string()));
        out.set("message", Json::Str(self.message.clone()));
        out
    }
}

/// What a frame is, to a pool: never the pixels (§47.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameIdentity {
    /// The stream this frame arrived on, e.g. `head_rgb`.
    pub stream: String,
    /// Acquisition timestamp, nanoseconds, from the sensor — never arrival (§47.2).
    pub stamp_ns: u64,
    /// Monotonic sequence within the stream. Gaps are drops, and drops are counted.
    pub sequence: u64,
}

impl FrameIdentity {
    /// `{"sequence": …, "stamp_ns": …, "stream": …}`.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("sequence", Json::Num(self.sequence as f64));
        out.set("stamp_ns", Json::Num(self.stamp_ns as f64));
        out.set("stream", Json::Str(self.stream.clone()));
        out
    }
}

/// The loan itself: one generation of one slot of one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHandle {
    /// Which stream's pool issued it.
    pub stream: String,
    /// Index into the pool's slots.
    pub slot: usize,
    /// The generation of the slot this handle was issued against.
    pub generation: u64,
}

impl FrameHandle {
    /// `{"generation": …, "slot": …, "stream": …}`.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("generation", Json::Num(self.generation as f64));
        out.set("slot", Json::Num(self.slot as f64));
        out.set("stream", Json::Str(self.stream.clone()));
        out
    }
}

/// What the pool has done, which is the only place a drop is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FramePoolStats {
    /// Frames stored.
    pub published: u64,
    /// Frames dropped because every slot was acquired. Recorded, never silent (§47.4).
    pub dropped: u64,
    /// Loans taken.
    pub acquired: u64,
    /// Loans returned.
    pub released: u64,
}

impl FramePoolStats {
    /// `{"acquired": …, "dropped": …, "published": …, "released": …}`.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("acquired", Json::Num(self.acquired as f64));
        out.set("dropped", Json::Num(self.dropped as f64));
        out.set("published", Json::Num(self.published as f64));
        out.set("released", Json::Num(self.released as f64));
        out
    }
}

#[derive(Debug, Clone, Default)]
struct Slot {
    generation: u64,
    identity: Option<FrameIdentity>,
    acquired: bool,
}

/// One stream's fixed frame pool. Pure bookkeeping: no clocks, no I/O, no mapping.
#[derive(Debug, Clone)]
pub struct FramePool {
    stream: String,
    slots: Vec<Slot>,
    stats: FramePoolStats,
}

impl FramePool {
    /// A pool of `capacity` slots for one named stream.
    ///
    /// Returns `None` for an unnamed stream or a capacity below one, which the reference raises
    /// rather than returns. Both are construction errors a descriptor causes, not conditions a
    /// running pool can reach, and the corpus does not script them for that reason: what a *robot*
    /// does with a sensor declaring zero frame slots is §5's validation, one layer up.
    pub fn new(stream: &str, capacity: usize) -> Option<FramePool> {
        if stream.is_empty() || capacity < 1 {
            return None;
        }
        let mut slots = Vec::new();
        slots.resize(capacity, Slot::default());
        Some(FramePool { stream: stream.to_string(), slots, stats: FramePoolStats::default() })
    }

    /// The stream this pool holds.
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// How many slots it has.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// What it has done.
    pub fn stats(&self) -> FramePoolStats {
        self.stats
    }

    /// Which slot the next publish takes: the released one holding the oldest frame.
    ///
    /// This is the entire mechanism by which a pool is a pool, and the obvious implementation —
    /// the first slot that is not currently acquired — is wrong in a way no single-publish test
    /// can see. With nothing acquired it returns slot 0 every time, so a pool of any declared
    /// capacity uses one slot; §47.4 already has a one-slot latest-wins channel, and a §47.6 that
    /// degenerated into it would be a duplicate of the section above it.
    ///
    /// The cost lands on the reader rather than on capture, which is why it presents as somebody
    /// else's bug: a policy holds a frame by calling [`latest`](Self::latest) and then
    /// [`acquire`](Self::acquire), and between those two calls capture publishes. Taking the first
    /// free slot means that publish lands on the slot `latest` just named, so the acquire is
    /// refused [`StaleHandle`](FrameRefusalCode::StaleHandle) on nearly every cycle.
    ///
    /// Empty slots go first because there is nothing in them to destroy, which is what makes a
    /// declared capacity mean what it says. After that, oldest is by the frame's **sequence** and
    /// never by the slot's index — the two stop agreeing the first time a reader holds a frame and
    /// lets it go. Ties break to the lower index so two editions cannot disagree.
    fn oldest_released(&self) -> Option<usize> {
        let mut chosen = None;
        let mut chosen_sequence = u64::MAX;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.acquired {
                continue;
            }
            match &slot.identity {
                // An empty slot has nothing to lose, so it wins outright and the search stops.
                None => return Some(index),
                Some(identity) => {
                    if identity.sequence < chosen_sequence {
                        chosen_sequence = identity.sequence;
                        chosen = Some(index);
                    }
                }
            }
        }
        chosen
    }

    /// Store a frame, returning its handle.
    ///
    /// Reuses the released slot holding the oldest frame (see [`oldest_released`](Self::oldest_released)),
    /// and when no slot is released the frame is dropped and counted rather than stalling capture.
    pub fn publish(&mut self, frame: FrameIdentity) -> Result<FrameHandle, FrameRefusal> {
        if frame.stream != self.stream {
            return Err(FrameRefusal {
                code: FrameRefusalCode::UnknownStream,
                message: format!(
                    "This pool holds {} and was handed a frame from {}.",
                    self.stream, frame.stream
                ),
            });
        }
        let Some(index) = self.oldest_released() else {
            self.stats.dropped += 1;
            return Err(FrameRefusal {
                code: FrameRefusalCode::PoolExhausted,
                message: format!(
                    "Every slot is acquired, so frame {} was dropped and counted rather than \
                     stalling capture.",
                    frame.sequence
                ),
            });
        };
        let slot = &mut self.slots[index];
        slot.generation += 1;
        slot.identity = Some(frame);
        slot.acquired = false;
        self.stats.published += 1;
        Ok(FrameHandle { stream: self.stream.clone(), slot: index, generation: slot.generation })
    }

    /// The newest published frame's handle, or `None` when nothing has been published yet.
    ///
    /// Never a frame of zeros: a zero frame is a measurement nobody took, and a policy handed one
    /// cannot tell it from a dark room.
    pub fn latest(&self) -> Option<FrameHandle> {
        let mut best = None;
        let mut best_sequence = 0u64;
        let mut seen = false;
        for (index, slot) in self.slots.iter().enumerate() {
            if let Some(identity) = &slot.identity {
                if !seen || identity.sequence > best_sequence {
                    seen = true;
                    best_sequence = identity.sequence;
                    best = Some(FrameHandle {
                        stream: self.stream.clone(),
                        slot: index,
                        generation: slot.generation,
                    });
                }
            }
        }
        best
    }

    /// Take a read loan on the named frame.
    ///
    /// The slot is pinned until [`release`](Self::release): publishing will reuse an older
    /// released slot first, and drop rather than reuse this one.
    pub fn acquire(&mut self, handle: &FrameHandle) -> Result<FrameIdentity, FrameRefusal> {
        let slot = self.locate(handle)?;
        if self.slots[slot].identity.is_none() || handle.generation != self.slots[slot].generation {
            return Err(FrameRefusal {
                code: FrameRefusalCode::StaleHandle,
                message: "The slot has been reused since this handle was issued, so the handle \
                          names a frame that is no longer there."
                    .to_string(),
            });
        }
        if self.slots[slot].acquired {
            return Err(FrameRefusal {
                code: FrameRefusalCode::AlreadyAcquired,
                message: "This handle is already checked out; a handle is one loan, not a share."
                    .to_string(),
            });
        }
        self.slots[slot].acquired = true;
        self.stats.acquired += 1;
        Ok(self.slots[slot].identity.clone().expect("checked above"))
    }

    /// Return a loan. Refuses unknown, stale and already-released handles by name.
    pub fn release(&mut self, handle: &FrameHandle) -> Result<(), FrameRefusal> {
        let slot = self.locate(handle)?;
        if self.slots[slot].identity.is_none() || handle.generation != self.slots[slot].generation {
            return Err(FrameRefusal {
                code: FrameRefusalCode::StaleHandle,
                message: "The slot has been reused since this handle was issued; there is nothing \
                          of it left to release."
                    .to_string(),
            });
        }
        if !self.slots[slot].acquired {
            return Err(FrameRefusal {
                code: FrameRefusalCode::DoubleRelease,
                message: "This handle was already released; releasing it twice would unpin a \
                          later loan."
                    .to_string(),
            });
        }
        self.slots[slot].acquired = false;
        self.stats.released += 1;
        Ok(())
    }

    /// The stream and slot checks both `acquire` and `release` open with.
    fn locate(&self, handle: &FrameHandle) -> Result<usize, FrameRefusal> {
        if handle.stream != self.stream || handle.slot >= self.slots.len() {
            return Err(FrameRefusal {
                code: FrameRefusalCode::UnknownSlot,
                message: "The handle names a slot this pool does not hold.".to_string(),
            });
        }
        Ok(handle.slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn frame(sequence: u64) -> FrameIdentity {
        FrameIdentity {
            stream: "head_rgb".to_string(),
            stamp_ns: sequence * 33_333_333,
            sequence,
        }
    }

    fn pool(capacity: usize) -> FramePool {
        FramePool::new("head_rgb", capacity).expect("a pool of at least one slot")
    }

    /// The property the corpus was written for, asserted here so a reader of this file meets it.
    ///
    /// A pool of three slots must use three slots. The implementation that takes the first
    /// unacquired slot passes every other test in this module and puts all four frames in slot 0.
    #[test]
    fn four_frames_spread_over_three_slots() {
        let mut pool = pool(3);
        let slots: Vec<usize> = (1..=4)
            .map(|sequence| pool.publish(frame(sequence)).expect("published").slot)
            .collect();
        assert_eq!(slots, vec![0, 1, 2, 0]);
        assert_eq!(pool.stats().published, 4);
        assert_eq!(pool.stats().dropped, 0);
    }

    /// How a policy holds a frame, and the reason the selection rule is not a detail.
    ///
    /// `latest` then `acquire`, with capture publishing in between. There is no lock across those
    /// two calls and there cannot be one — capture must never wait on a reader — so the pool's
    /// only defence is publishing somewhere with less to lose.
    #[test]
    fn a_reader_that_looks_then_takes_gets_its_frame() {
        let mut pool = pool(3);
        pool.publish(frame(1)).expect("published");
        let seen = pool.latest().expect("a frame");
        pool.publish(frame(2)).expect("published");
        let taken = pool.acquire(&seen).expect("the frame it named");
        assert_eq!(taken.sequence, 1);
    }

    /// Slot order and arrival order stop agreeing the moment a reader has been through.
    #[test]
    fn released_slots_come_back_oldest_first() {
        let mut pool = pool(3);
        let first = pool.publish(frame(1)).expect("published");
        let second = pool.publish(frame(2)).expect("published");
        pool.acquire(&first).expect("loan");
        pool.acquire(&second).expect("loan");
        pool.release(&second).expect("returned");
        pool.release(&first).expect("returned");

        // Slot 2 is empty and has nothing to lose, so it goes first.
        assert_eq!(pool.publish(frame(3)).expect("published").slot, 2);
        // Then the oldest *frame*, which is in slot 0 — not the slot released most recently.
        assert_eq!(pool.publish(frame(4)).expect("published").slot, 0);
    }

    #[test]
    fn an_empty_pool_is_not_a_frame_of_zeros() {
        assert!(pool(2).latest().is_none());
    }

    #[test]
    fn a_stale_handle_never_shows_another_frame() {
        let mut pool = pool(1);
        let first = pool.publish(frame(1)).expect("published");
        pool.acquire(&first).expect("loan");
        pool.release(&first).expect("returned");
        let second = pool.publish(frame(2)).expect("published");
        assert_ne!(first.generation, second.generation);

        let refused = pool.acquire(&first).expect_err("the slot has been reused");
        assert_eq!(refused.code, FrameRefusalCode::StaleHandle);
        assert_eq!(pool.acquire(&second).expect("the new frame").sequence, 2);
    }

    #[test]
    fn a_pinned_slot_drops_and_counts_rather_than_stalling() {
        let mut pool = pool(1);
        let first = pool.publish(frame(1)).expect("published");
        pool.acquire(&first).expect("loan");

        let refused = pool.publish(frame(2)).expect_err("every slot is acquired");
        assert_eq!(refused.code, FrameRefusalCode::PoolExhausted);
        assert_eq!(pool.stats().dropped, 1);
        // The drop changed nothing a reader can see except the counter.
        assert_eq!(pool.latest().expect("a frame"), first);
    }

    #[test]
    fn a_loan_is_not_a_share() {
        let mut pool = pool(2);
        let handle = pool.publish(frame(1)).expect("published");
        pool.acquire(&handle).expect("loan");
        assert_eq!(
            pool.acquire(&handle).expect_err("already out").code,
            FrameRefusalCode::AlreadyAcquired
        );
        pool.release(&handle).expect("returned");
        assert_eq!(
            pool.release(&handle).expect_err("already back").code,
            FrameRefusalCode::DoubleRelease
        );
    }

    #[test]
    fn handles_and_frames_that_name_something_else_are_refused() {
        let mut pool = pool(2);
        let elsewhere = FrameIdentity {
            stream: "wrist_cam".to_string(),
            stamp_ns: 0,
            sequence: 1,
        };
        assert_eq!(
            pool.publish(elsewhere).expect_err("another stream").code,
            FrameRefusalCode::UnknownStream
        );
        let out_of_range = FrameHandle {
            stream: "head_rgb".to_string(),
            slot: 7,
            generation: 1,
        };
        assert_eq!(
            pool.acquire(&out_of_range).expect_err("no such slot").code,
            FrameRefusalCode::UnknownSlot
        );
        let other_stream = FrameHandle {
            stream: "other".to_string(),
            slot: 0,
            generation: 1,
        };
        assert_eq!(
            pool.release(&other_stream).expect_err("another pool").code,
            FrameRefusalCode::UnknownSlot
        );
    }

    /// A capacity of zero is a construction error, not a pool that drops everything.
    ///
    /// The reference throws; this returns `None`. Both refuse, and neither produces a pool that
    /// would answer `pool_exhausted` for ever and read as a camera under load.
    #[test]
    fn a_pool_needs_a_name_and_a_slot() {
        assert!(FramePool::new("", 4).is_none());
        assert!(FramePool::new("head_rgb", 0).is_none());
    }

    #[test]
    fn the_refusal_vocabulary_is_the_corpuss() {
        assert_eq!(FRAME_REFUSAL_CODES.len(), 6);
        for code in [
            FrameRefusalCode::UnknownStream,
            FrameRefusalCode::UnknownSlot,
            FrameRefusalCode::StaleHandle,
            FrameRefusalCode::AlreadyAcquired,
            FrameRefusalCode::DoubleRelease,
            FrameRefusalCode::PoolExhausted,
        ] {
            assert!(FRAME_REFUSAL_CODES.contains(&code.as_str()));
        }
    }
}
