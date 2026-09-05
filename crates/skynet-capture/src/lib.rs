#![no_std]
#![forbid(unsafe_code)]
//! The capture-side split (§47.4).
//!
//! §47.4 is titled *"Two consumers, and they must not share a queue"*, and it calls itself **the
//! design decision in this section**. This crate is that decision, and nothing else:
//!
//! | Consumer | Wants | Gets |
//! |---|---|---|
//! | **Policy** | The freshest frame; an old frame is worthless | A [`LatestSlot`]. New frames overwrite unconsumed ones. |
//! | **Black box** | Every frame; completeness is the guarantee | A bounded [`RecordQueue`] feeding the encoder |
//!
//! > A single queue serving both is the classic error, and it fails in the direction nobody wants:
//! > the policy ends up reading queued frames, so a momentary encoder stall becomes permanent added
//! > latency in the control path. A queue in front of a policy is latency wearing the costume of
//! > reliability.
//!
//! The two disciplines are opposite on purpose. The slot **discards** to stay fresh; the queue
//! **retains** to stay complete. Neither can stall capture, because §47.4 says so in a sentence
//! that is really about priority: *"Capture is never stalled, because stalling capture starves the
//! policy to protect the recording, which is precisely backwards."*
//!
//! # Why this crate holds no pixels
//!
//! §47.3 passes capture buffers as `dmabuf` handles by reference, from the capture device to the
//! GPU and the encoder, because three 1080p30 streams copied through userspace is roughly a core of
//! `memcpy` spent achieving nothing. So what moves through here is a [`Captured`] — a handle, a
//! stream index, a sequence and an acquisition stamp — and the bytes are never touched.
//!
//! That is not only a performance decision. It is what makes §47.4's actual claim testable with no
//! camera, no V4L2, no GPU and no encoder: the discipline is about *which frame each consumer sees
//! and what happens when one falls behind*, and none of that involves a pixel.
//!
//! # Somebody has to release the buffer, and this crate says who
//!
//! A zero-copy pipeline has exactly one hard rule: a buffer handed onward must be handed back, or
//! the pool runs dry and capture stops for a reason that looks like a driver fault. So every
//! operation here that displaces or discards a frame **returns it**:
//!
//! * [`LatestSlot::publish`] returns the frame it overwrote, if any.
//! * [`LatestSlot::take`] returns the frame, and the slot no longer holds it.
//! * [`RecordQueue::push`] returns [`Push::Dropped`] carrying the frame it refused.
//! * [`RecordQueue::pop`] returns the frame the encoder should now read.
//!
//! An API that swallowed them would be one where the leak is invisible in the type system and
//! obvious three hours in, which is §47.5's description of an unbounded capture pool.
//!
//! # `no_std`, no `alloc`, fixed capacity
//!
//! For the reason `skynet-ring` gives: this sits in the path a control loop runs, and an allocation
//! there is a latency spike nobody scheduled. Capacity is a const parameter, declared per stream as
//! §47.5 requires, so the pool is bounded by construction rather than by a policy somebody enforces.

// ── What moves through the pipeline ──────────────────────────────────────────

/// Where a frame's timestamp came from (§47.2).
///
/// **Timestamps come from the sensor, not from arrival.** V4L2 offers
/// `V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC` and it is used; GenICam offers a device timestamp and it is
/// used. Where a backend genuinely cannot supply one, the pipeline records an *estimated* latency
/// and the frame carries a larger uncertainty, which flows into the skew §25.1's assembler already
/// reports.
///
/// The variant that does not exist is the important one. There is no `Arrival`, because §47.2 is
/// explicit that *"estimating is acceptable; silently substituting arrival time is not"* — it folds
/// a variable queue delay into a number the policy treats as ground truth. A backend that has only
/// an arrival time must say [`StampSource::Estimated`] and name its uncertainty, which is the same
/// information told honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampSource {
    /// The device stamped it, at acquisition.
    Device,
    /// The pipeline estimated it, and `uncertainty_ns` on the frame says by how much.
    Estimated,
}

/// One captured buffer, by reference (§47.3).
///
/// Sixteen bytes of metadata and a handle. Nothing here dereferences `handle`; on Linux it is a
/// `dmabuf` file descriptor and on a test it is a number, and this crate cannot tell the difference
/// because it never needs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Captured {
    /// Which stream this came from, as an index into the descriptor's `sensors`.
    pub stream: u16,
    /// Per-stream sequence, assigned by capture and never reused.
    ///
    /// The black box's evidence that a gap is a gap: two consecutive records whose sequences differ
    /// by more than one is a drop, whether or not anything counted it.
    pub seq: u64,
    /// Acquisition timestamp, `CLOCK_MONOTONIC_RAW` nanoseconds (§35.1, §47.2). Never arrival.
    pub stamp_ns: u64,
    /// How much this stamp could be wrong by.
    ///
    /// Zero from a device that stamps at acquisition. Non-zero from an estimate, and it flows into
    /// the skew §25.1's assembler reports rather than being discarded here — a frame whose time is
    /// uncertain by 20 ms is usable, and a frame whose time is uncertain by 20 ms and says it is
    /// exact is the thing that sends somebody to retrain a model that was fine.
    pub uncertainty_ns: u64,
    /// The buffer, by reference. Never dereferenced in this crate.
    pub handle: u64,
    /// Whether the stamp was measured or estimated.
    pub stamp: StampSource,
}

impl Captured {
    /// A frame stamped by its device.
    pub fn device(stream: u16, seq: u64, stamp_ns: u64, handle: u64) -> Captured {
        Captured {
            stream,
            seq,
            stamp_ns,
            uncertainty_ns: 0,
            handle,
            stamp: StampSource::Device,
        }
    }

    /// A frame whose stamp is an estimate, with the uncertainty that makes it honest.
    ///
    /// There is no constructor that produces an estimate with zero uncertainty, because that is
    /// exactly the silent substitution §47.2 forbids: it would present a guess in the shape of a
    /// measurement. A backend with no idea of its own latency has to write a number down.
    pub fn estimated(
        stream: u16,
        seq: u64,
        stamp_ns: u64,
        uncertainty_ns: core::num::NonZeroU64,
        handle: u64,
    ) -> Captured {
        Captured {
            stream,
            seq,
            stamp_ns,
            uncertainty_ns: uncertainty_ns.get(),
            handle,
            stamp: StampSource::Estimated,
        }
    }
}

// ── The policy's side: latest wins ───────────────────────────────────────────

/// The freshest frame, and only the freshest (§47.4).
///
/// One slot. Publishing into a full slot **overwrites** and returns what was there, so a policy that
/// took 40 ms over a cycle reads the frame from the end of that 40 ms rather than the one from the
/// start plus a queue of stale ones behind it.
///
/// This is a single-producer single-consumer structure used from one thread or behind whatever
/// synchronisation the runtime already holds. It is deliberately **not** atomic: the ring that
/// crosses a process boundary is `skynet-ring`, which pays for a seqlock because it must; this sits
/// on one side of that boundary and paying twice would be paying for nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct LatestSlot {
    held: Option<Captured>,
    /// Frames overwritten before anyone read them, for the life of the slot.
    overwritten: u64,
}

impl LatestSlot {
    /// An empty slot.
    pub const fn new() -> LatestSlot {
        LatestSlot {
            held: None,
            overwritten: 0,
        }
    }

    /// Publish a frame, returning the one it displaced.
    ///
    /// Never fails and never blocks — that is the whole discipline. The returned frame is the
    /// caller's to release: it is no longer reachable from here, and dropping it on the floor is
    /// how a `dmabuf` pool runs dry.
    pub fn publish(&mut self, frame: Captured) -> Option<Captured> {
        let displaced = self.held.replace(frame);
        if displaced.is_some() {
            self.overwritten = self.overwritten.saturating_add(1);
        }
        displaced
    }

    /// Take the held frame, leaving the slot empty.
    ///
    /// Taking rather than peeking, because a policy that read the same buffer twice would run one
    /// cycle on a frame it had already acted on — and because the handle has to reach exactly one
    /// owner for the release rule above to hold.
    pub fn take(&mut self) -> Option<Captured> {
        self.held.take()
    }

    /// What is held, without taking it. For a status panel, not for a control loop.
    pub fn peek(&self) -> Option<&Captured> {
        self.held.as_ref()
    }

    /// Whether a frame is waiting.
    pub fn is_full(&self) -> bool {
        self.held.is_some()
    }

    /// Frames overwritten before anyone read them.
    ///
    /// **Not a fault, and not nothing.** A steady count means capture is faster than the policy,
    /// which is the intended arrangement — §47.4 wants the policy on the freshest frame, and being
    /// on the freshest frame *is* discarding the ones before it. A count that suddenly climbs is a
    /// policy that has slowed down, and it is the earliest evidence of that available anywhere,
    /// because it appears before the control rate does.
    pub fn overwritten(&self) -> u64 {
        self.overwritten
    }
}

// ── The black box's side: bounded, complete, and honest about gaps ───────────

/// What a push did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// Queued. The encoder will get it.
    Accepted,
    /// Refused, and here is the frame back.
    ///
    /// The queue is full, which means the encoder has fallen behind. §47.4: *"recording frames are
    /// dropped **and the drop is recorded**"* — capture is not stalled to protect the recording.
    /// The frame comes back because somebody has to release the buffer.
    Dropped(Captured),
}

/// A gap in the recording, annotated (§47.4).
///
/// > A gap in the black box that is annotated as a gap is honest; a gap that looks like a quiet
/// > period is not.
///
/// This is the annotation. A run of dropped frames collapses into one of these — first and last
/// sequence, first and last stamp, and the count — so the black box records *"41 frames of stream 2
/// are missing, between these two instants"* rather than nothing at all, which is what a reader
/// would otherwise have to infer from a sequence jump they might not look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// Which stream lost frames.
    pub stream: u16,
    /// Sequence of the first frame dropped.
    pub from_seq: u64,
    /// Sequence of the last frame dropped.
    pub to_seq: u64,
    /// Acquisition stamp of the first frame dropped.
    pub from_ns: u64,
    /// Acquisition stamp of the last frame dropped.
    pub to_ns: u64,
    /// How many frames were dropped. Never zero: a gap of nothing is not a gap.
    pub frames: u64,
}

/// Every frame, up to `N`, and a recorded gap when it cannot be (§47.4).
///
/// Bounded by construction — `N` is the per-stream declaration §47.5 asks for, and a pool that
/// cannot grow is a pool that cannot become the slow leak §47.5 describes, the one that *"presents
/// as a policy OOM three hours in"*.
///
/// Backpressure is resolved **here and not at capture**. A full queue refuses, hands the frame back
/// and opens or extends a [`Gap`]; it never waits for the encoder, because waiting would move the
/// encoder's stall into the capture path and from there into the policy's, which is the failure
/// §47.4 exists to prevent.
#[derive(Debug)]
pub struct RecordQueue<const N: usize> {
    slots: [Option<Captured>; N],
    head: usize,
    len: usize,
    /// The gap currently open, if the last push was a drop.
    open: Option<Gap>,
    /// Frames dropped for the life of the queue.
    dropped: u64,
    /// Gaps closed, for the life of the queue.
    gaps: u64,
}

impl<const N: usize> Default for RecordQueue<N> {
    fn default() -> RecordQueue<N> {
        RecordQueue::new()
    }
}

impl<const N: usize> RecordQueue<N> {
    /// An empty queue of `N` frames.
    ///
    /// `N` of zero is legitimate and means *"do not record this stream"*: every push is refused and
    /// every refusal is recorded, so the black box holds an honest account of a stream nobody kept
    /// rather than no account of it. It is not an error, because a robot with no storage for a
    /// 1080p stream is a real configuration and the alternative is a panic in a capture path.
    pub const fn new() -> RecordQueue<N> {
        RecordQueue {
            slots: [None; N],
            head: 0,
            len: 0,
            open: None,
            dropped: 0,
            gaps: 0,
        }
    }

    /// Offer a frame to the recorder.
    ///
    /// Never blocks. Returns [`Push::Dropped`] with the frame when the encoder has fallen behind,
    /// and opens or extends a gap so the loss is in the record rather than only in the counters.
    pub fn push(&mut self, frame: Captured) -> Push {
        if self.len == N {
            self.dropped = self.dropped.saturating_add(1);
            match &mut self.open {
                // Extending, not replacing. A hundred consecutive drops are one gap with a hundred
                // frames in it, which is what a reader needs; a hundred gaps of one frame each is
                // the same information rendered unreadable.
                Some(gap) if gap.stream == frame.stream => {
                    gap.to_seq = frame.seq;
                    gap.to_ns = frame.stamp_ns;
                    gap.frames = gap.frames.saturating_add(1);
                }
                _ => {
                    self.open = Some(Gap {
                        stream: frame.stream,
                        from_seq: frame.seq,
                        to_seq: frame.seq,
                        from_ns: frame.stamp_ns,
                        to_ns: frame.stamp_ns,
                        frames: 1,
                    });
                }
            }
            return Push::Dropped(frame);
        }

        let index = (self.head + self.len) % N.max(1);
        self.slots[index] = Some(frame);
        self.len += 1;
        Push::Accepted
    }

    /// Take the oldest frame, for the encoder.
    ///
    /// Oldest first, unlike the slot: the black box's guarantee is completeness and order, and a
    /// recording that dropped its oldest frame to make room for a newer one would be a recording
    /// that rewrote its own past under load.
    pub fn pop(&mut self) -> Option<Captured> {
        if self.len == 0 {
            return None;
        }
        let frame = self.slots[self.head].take();
        self.head = (self.head + 1) % N.max(1);
        self.len -= 1;
        frame
    }

    /// Close the open gap, if there is one, and hand it to the recorder.
    ///
    /// Called when a push succeeds again, or at the end of a recording. Returns `None` when nothing
    /// was lost, which is the common case and is why this is a separate call rather than something
    /// `push` returns: a recorder that had to handle an `Option<Gap>` on every accepted frame would
    /// spend its whole life handling `None`.
    pub fn close_gap(&mut self) -> Option<Gap> {
        let gap = self.open.take();
        if gap.is_some() {
            self.gaps = self.gaps.saturating_add(1);
        }
        gap
    }

    /// The gap currently open, if the queue is losing frames right now.
    pub fn open_gap(&self) -> Option<&Gap> {
        self.open.as_ref()
    }

    /// Frames waiting for the encoder.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bound. Declared per stream (§47.5) and never exceeded.
    pub fn capacity(&self) -> usize {
        N
    }

    /// Frames the encoder never saw, for the life of the queue.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Gaps closed, for the life of the queue.
    ///
    /// Distinct from [`dropped`](RecordQueue::dropped) and both are worth having: two hundred frames
    /// lost in one gap is a stall, and two hundred lost in two hundred gaps is a queue sized one
    /// frame too small. They call for completely different work.
    pub fn gaps(&self) -> u64 {
        self.gaps
    }
}

// ── The two together, which is the point ─────────────────────────────────────

/// One stream's capture-side distribution: the slot and the queue, fed once (§47.4).
///
/// The type exists so that the *"they must not share a queue"* rule is expressed as a shape rather
/// than as a convention. A caller holding this cannot accidentally feed the policy from the
/// recorder's queue, because there is one entry point and it hands the frame to both disciplines.
#[derive(Debug)]
pub struct Distributor<const N: usize> {
    /// The policy's freshest frame.
    pub latest: LatestSlot,
    /// The black box's bounded queue.
    pub record: RecordQueue<N>,
}

/// What one captured frame cost, once both consumers had seen it.
///
/// Every handle that left the pipeline is in here. A caller that releases
/// [`displaced`](Delivered::displaced) and [`not_recorded`](Delivered::not_recorded) has released
/// everything this call took ownership of and nothing it still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Delivered {
    /// The frame the policy had not read yet, overwritten by this one.
    pub displaced: Option<Captured>,
    /// This frame, when the recorder had no room for it.
    ///
    /// `Some` here does **not** mean the policy missed it: the slot always takes it. The two
    /// consumers fail independently, which is the whole of §47.4.
    pub not_recorded: Option<Captured>,
}

impl<const N: usize> Default for Distributor<N> {
    fn default() -> Distributor<N> {
        Distributor::new()
    }
}

impl<const N: usize> Distributor<N> {
    /// A distributor with an empty slot and an empty queue.
    pub const fn new() -> Distributor<N> {
        Distributor {
            latest: LatestSlot::new(),
            record: RecordQueue::new(),
        }
    }

    /// Hand one captured frame to both consumers.
    ///
    /// **Never fails, never blocks, never stalls capture.** That is the invariant §47.4 states and
    /// the one thing this function has to guarantee: whatever the encoder is doing and whatever the
    /// policy is doing, the caller returns and captures the next frame.
    ///
    /// The recorder is offered the frame **first**, and the order is not arbitrary. The slot's
    /// `publish` consumes the frame by value, so offering the slot first would mean the recorder
    /// only ever saw a copy of something already published — correct today, because `Captured` is
    /// `Copy`, and a trap the moment a handle grows a destructor. Doing it in the order that stays
    /// correct under that change costs nothing now.
    pub fn deliver(&mut self, frame: Captured) -> Delivered {
        let not_recorded = match self.record.push(frame) {
            Push::Accepted => None,
            Push::Dropped(frame) => Some(frame),
        };
        Delivered {
            displaced: self.latest.publish(frame),
            not_recorded,
        }
    }
}

#[cfg(test)]
mod tests;
