//! §25's observation ring: the mapping that carries the robot's body back to the policy.
//!
//! ```text
//! Policy backend                       Safety Kernel                     RHAL
//! ──────────────                       ─────────────                     ────
//!               ┌──────────────────┐
//!               │ THIS RING        │ ◀── state ◀────────────────────── read_state()
//!               │ observations     │
//!               └──────────────────┘
//! desired[]  ──▶ ┌──────────────────┐
//!                │ skynet-ring      │ ──▶ 8 guards ──▶ rhal_command ──▶ write_command()
//!                └──────────────────┘
//! ```
//!
//! §25 specifies the upward arrow — *"the ring is the only channel **from the policy to the
//! kernel**"* — and says nothing about the downward one except that the policy *"cannot reach the
//! RHAL at all"*. That left a hole big enough to stop the whole left-hand column: `skynet-contracts`
//! has had §25.1's assembler, corpus-pinned in three editions, for as long as there has been a
//! corpus, and it was wired to nothing because no observation could get to a process that could use
//! it. Every policy backend was open-loop.
//!
//! This is that channel. It is the mirror of `skynet-ring` and deliberately not the same ring:
//! single-producer, single-consumer, lock-free, one page per slot, no allocation, no blocking.
//!
//! # Why a separate crate rather than a second record in `skynet-ring`
//!
//! The protocol is the same and the duplication is real — about a hundred and fifty lines of
//! seqlock. It buys one thing, and the thing is worth more than the lines:
//!
//! **The magic differs, so the two rings cannot be confused by construction.** A consumer built for
//! commands that opened an observation mapping would be reading joint *positions* as commanded
//! setpoints, under a valid header, with the drives live. With `SKYO`/`BSV1` here and `SKYR`/`ING1`
//! there, [`Ring::open`] refuses at the first word. The alternative — one crate, one header, a
//! `kind` field distinguishing them — puts that check one field further in, on a value a caller
//! passes, in the crate that is on the safety path. This way the type system and the magic agree,
//! and `skynet-ring` was not touched to get it.
//!
//! [`tests::the_geometry_matches_the_command_rings`] is what keeps the two formats' shared constants
//! shared. A comment saying they should agree would not.
//!
//! # What it carries, and what it does not
//!
//! **Proprioception.** Joint positions, velocities and efforts, the base pose and twist, the driver
//! flags and fault code, and the acquisition stamp — which is [`Observation::stamp_ns`], and is the
//! stamp §24 insists on: *at acquisition, never at return*. That is the whole of what §25.1's
//! assembler needs to place a joint sample in a frame, and it fits a page with room to spare.
//!
//! **Not camera frames.** A 4 KiB slot holds 128 joints three times over and does not hold one
//! 640×480 image, and widening the slot is not the answer — a ring of megabyte slots is a copy per
//! frame at capture rate. Vision needs a buffer pool with handles and a lifetime rule, which is a
//! larger design than this one and should not be started as a side effect of it. So a policy driven
//! by this ring can close the loop on the body and not on what the body can see, and the honest
//! consequence is that manipulation from pixels is still out of reach. §80 carries it.
//!
//! # The direction is what makes it cheap
//!
//! The process that owns the RHAL already reads the full state every cycle, because §7's guards
//! need it. Publishing it here is a copy into a mapping it already holds — no extra driver call, no
//! extra allocation, and nothing on the path between a policy and a joint.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use core::sync::atomic::{AtomicU32, Ordering};

/// The version of this layout. Bumped only for a breaking change.
///
/// A producer and a consumer that disagree here do not negotiate, for the reason `skynet-ring` gives
/// about its own: half a shared-memory protocol is worse than none.
///
/// **2** since the header carries the ring's [`TimeBase`] (`DRY-COVERAGE-PLAN.md` F70). The slot did
/// not change and the break is still real: a version-1 reader takes every stamp as
/// `CLOCK_MONOTONIC_RAW`, which on a simulated body is the wrong clock — the one misreading this
/// field exists to prevent — so it is refused at the version rather than left to find out.
pub const ABI_VERSION: u32 = 2;

/// `"SKYO"`, little-endian. Deliberately not `skynet-ring`'s `"SKYR"`.
const MAGIC_0: u32 = u32::from_le_bytes(*b"SKYO");
/// `"BSV1"`, little-endian.
const MAGIC_1: u32 = u32::from_le_bytes(*b"BSV1");

/// Bytes in a slot, and in the header. One page, for the reason `skynet-ring` gives.
pub const SLOT_BYTES: usize = 4096;

/// `u32` words in a slot.
pub const SLOT_WORDS: usize = SLOT_BYTES / 4;

/// Bytes of header before the first slot.
pub const HEADER_BYTES: usize = 4096;

/// `u32` words in the header.
pub const HEADER_WORDS: usize = HEADER_BYTES / 4;

/// The most joints this ring can carry.
///
/// Equal to `skynet_ring::MAX_CHANNELS` and, as there, deliberately not imported from it: a ring is
/// a place two processes meet rather than a place another crate's opinions apply.
/// [`tests::the_geometry_matches_the_command_rings`] keeps them equal.
pub const MAX_JOINTS: usize = 128;

/// Which clock every stamp in this ring is on — and therefore which clock the kernel's time is.
///
/// §35.3: *"because the kernel's time is supplied by the RHAL rather than read from the OS, sim time
/// simply is the kernel's time"*. On a real body the two coincide, because a driver stamps on
/// §35.1's clock. On a simulator they do not: `drivers/mujoco` stamps a state with the simulation's
/// own time, which starts at zero and runs at whatever speed the physics does. So the process that
/// holds the driver decides it once, from the body's own descriptor, and writes it here — the one
/// place both processes read — because a policy that aged a simulated stamp against
/// `CLOCK_MONOTONIC_RAW` computes an age the size of the machine's uptime (`DRY-COVERAGE-PLAN.md`
/// F70), and a policy has no way to tell which clock a bare number is on.
///
/// Written at [`Ring::create`] and never again: a body does not change what it is mid-run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeBase {
    /// §35.1's clock: `CLOCK_MONOTONIC_RAW`, absolute, which every process on the machine reads.
    Monotonic,
    /// §35.3's: the body is a simulator, its stamps are its own time, and the kernel's `now` on each
    /// cycle is the stamp of the state that cycle read. A process on the other side of the ring
    /// has no clock of its own to read it from; the newest stamp published here is the time.
    Simulated,
}

impl TimeBase {
    /// The word written into the header. Zero is deliberately neither: an unwritten word is not a
    /// claim about either clock, and [`Ring::open`] refuses it rather than reading it as the first.
    const fn code(self) -> u32 {
        match self {
            TimeBase::Monotonic => 1,
            TimeBase::Simulated => 2,
        }
    }

    const fn from_code(code: u32) -> Option<TimeBase> {
        match code {
            1 => Some(TimeBase::Monotonic),
            2 => Some(TimeBase::Simulated),
            _ => None,
        }
    }

    /// What a person reads: which clock, and the section that says why.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeBase::Monotonic => "CLOCK_MONOTONIC_RAW (§35.1)",
            TimeBase::Simulated => "the simulator's own clock (§35.3)",
        }
    }
}

mod header {
    pub const MAGIC_0: usize = 0;
    pub const MAGIC_1: usize = 1;
    pub const ABI_VERSION: usize = 2;
    pub const SLOT_WORDS: usize = 3;
    pub const SLOT_COUNT: usize = 4;
    pub const MAX_JOINTS: usize = 5;
    /// [`super::TimeBase`]'s code. Written once, before the magic, like the rest of the geometry.
    pub const TIME_BASE: usize = 6;
    /// On its own cache line, 64 bytes in. The one field that moves.
    pub const PUBLISHED: usize = 16;
}

mod slot {
    pub const SEQLOCK: usize = 0;
    pub const STAMP_LO: usize = 2;
    pub const STAMP_HI: usize = 3;
    pub const SEQ_LO: usize = 4;
    pub const SEQ_HI: usize = 5;
    pub const JOINTS: usize = 6;
    pub const FLAGS: usize = 7;
    pub const FAULT_CODE: usize = 8;
    /// `x y z qx qy qz qw`.
    pub const BASE_POSE: usize = 9;
    /// `vx vy vz wx wy wz`.
    pub const BASE_TWIST: usize = 16;
    pub const POSITION: usize = 22;
}

const VELOCITY_AT: usize = slot::POSITION + MAX_JOINTS;
const EFFORT_AT: usize = VELOCITY_AT + MAX_JOINTS;
const SLOT_USED_WORDS: usize = EFFORT_AT + MAX_JOINTS;

/// A slot must fit in its page, and this is the compiler's business rather than a test's.
///
/// The same build-time assertion `skynet-ring` makes, for the same failure: a publish that wrote
/// past the end of one slot would land in the head of the next, producing a torn observation on a
/// neighbouring slot with a perfectly valid seqlock — the one corruption this format's reader
/// cannot see.
const _: () = assert!(
    SLOT_USED_WORDS <= SLOT_WORDS,
    "an observation's fields do not fit in SLOT_BYTES"
);

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsError {
    /// The region is too small, or the slot count is not a power of two.
    Geometry,
    /// The header is not one of these rings.
    ///
    /// Which is the error a process gets for opening a **command** ring here, or the reverse, and
    /// it is the whole reason this crate has its own magic.
    NotARing,
    /// The header is a ring of a version this build does not speak.
    Version,
    /// More joints than [`MAX_JOINTS`].
    TooManyJoints,
    /// The header names a clock this build does not know, or none.
    ///
    /// Its own variant rather than [`ObsError::Geometry`], because the remedy is different: a
    /// geometry fault is a mapping of the wrong size, and this is two builds disagreeing about what
    /// a stamp means — which is the disagreement the field was added to make impossible to miss.
    TimeBase,
}

impl ObsError {
    /// A short sentence naming the fault.
    pub fn as_str(self) -> &'static str {
        match self {
            ObsError::Geometry => {
                "the region is too small, or the slot count is not a power of two"
            }
            ObsError::NotARing => "this mapping is not an observation ring",
            ObsError::Version => "this observation ring is a version this build does not speak",
            ObsError::TooManyJoints => "more joints than this ring carries",
            ObsError::TimeBase => {
                "this observation ring names no clock this build knows, so its stamps cannot be \
                 aged (§35.3)"
            }
        }
    }
}

/// One sample of the robot's body.
///
/// Fixed-size and `Copy`, so a producer fills one on its stack and reuses it: §24's rule that the
/// hot path never touches an allocator applies on this side of the RHAL as much as the far side.
///
/// # No `PartialEq`, deliberately
///
/// Every entry beyond [`Self::joints`] is `NaN` - see the `Default` implementation - and `NaN` is
/// not equal to itself, so a derived comparison would report two reads of *the same slot* as
/// different. That is a comparison which is false exactly when it is most likely to be trusted, and
/// the one place anybody would reach for it is a test asserting that nothing changed. Compare the
/// fields that carry meaning: the stamp, the sequence, and the joints inside the count.
#[derive(Debug, Clone, Copy)]
pub struct Observation {
    /// **At acquisition**, never at return (§24), on the ring's [`TimeBase`] — `CLOCK_MONOTONIC_RAW`
    /// on a real body, the simulation's own time on a simulated one (§35.3).
    ///
    /// This is the field §25.1's assembler places a sample by, so a stamp applied when the
    /// observation was *published* rather than when the driver acquired it would move every frame
    /// this ring feeds, in the direction that hides skew.
    pub stamp_ns: u64,
    /// The state sample's sequence, as the driver reported it.
    pub seq: u64,
    /// How many entries of the three arrays carry meaning.
    pub joints: usize,
    /// Bitwise-or of `skynet_rhal::flags`, carried verbatim.
    pub flags: u32,
    /// Vendor-specific, carried verbatim.
    pub fault_code: u32,
    /// `x y z qx qy qz qw`; all `NaN` when the base is not localised.
    pub base_pose: [f32; 7],
    /// `vx vy vz wx wy wz`.
    pub base_twist: [f32; 6],
    /// Measured positions.
    pub position: [f32; MAX_JOINTS],
    /// Measured velocities; `NaN` where the joint does not measure it.
    pub velocity: [f32; MAX_JOINTS],
    /// Measured efforts; `NaN` where unmeasured.
    pub effort: [f32; MAX_JOINTS],
}

impl Default for Observation {
    fn default() -> Self {
        Observation {
            stamp_ns: 0,
            seq: 0,
            joints: 0,
            flags: 0,
            fault_code: 0,
            // `NaN`, not zero. An unlocalised base is what a default observation describes, and a
            // pose of all zeros is a specific and confident claim that the robot is at the origin.
            base_pose: [f32::NAN; 7],
            base_twist: [0.0; 6],
            // `NaN` for the same reason as the pose, and it is load-bearing rather than tidy. A
            // consumer reads only the joints a slot declares - 384 loads per cycle for a six-joint
            // arm would be most of the copy - so what a caller sees *above* the count is whatever
            // this default put there. Zero would be a measurement: a claim that joint seven of a
            // three-joint body is at its origin, under the fresh stamp of a real observation. `NaN`
            // is unusable, which is what "there is no measurement here" should feel like.
            position: [f32::NAN; MAX_JOINTS],
            velocity: [f32::NAN; MAX_JOINTS],
            effort: [f32::NAN; MAX_JOINTS],
        }
    }
}

/// What a consumer found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// A complete slot was read into the caller's buffer.
    Fresh,
    /// Nothing has ever been published.
    ///
    /// A policy that starts before the process holding the driver, which is normal and is not a
    /// fault. What a backend does about it is the backend's, and §25.3's answer for a policy that
    /// cannot decide is a hold.
    Empty,
    /// The producer overwrote the slot mid-read, repeatedly.
    Stale,
}

/// A ring, over a shared region of `u32` words.
#[derive(Debug, Clone, Copy)]
pub struct Ring<'a> {
    words: &'a [AtomicU32],
    slot_count: usize,
    /// Read once, at [`Ring::create`] or [`Ring::open`], and validated there — so a `Ring` that
    /// exists names a clock, and nothing downstream has a third case to handle.
    time: TimeBase,
}

/// How many words a region must hold for a ring of `slot_count` slots.
pub const fn words_for(slot_count: usize) -> usize {
    HEADER_WORDS + slot_count * SLOT_WORDS
}

impl<'a> Ring<'a> {
    /// Write a fresh header over `words` and return the ring it describes.
    ///
    /// `time` is required, with no default, because the default would be a claim: a ring created
    /// without saying which clock it carries would read as `CLOCK_MONOTONIC_RAW`, which is exactly
    /// the misreading of a simulator's stamps that F70 was.
    pub fn create(
        words: &'a [AtomicU32],
        slot_count: usize,
        time: TimeBase,
    ) -> Result<Ring<'a>, ObsError> {
        if slot_count == 0 || !slot_count.is_power_of_two() {
            return Err(ObsError::Geometry);
        }
        if words.len() < words_for(slot_count) {
            return Err(ObsError::Geometry);
        }
        for word in words.iter().take(words_for(slot_count)) {
            word.store(0, Ordering::Relaxed);
        }
        words[header::SLOT_WORDS].store(SLOT_WORDS as u32, Ordering::Relaxed);
        words[header::SLOT_COUNT].store(slot_count as u32, Ordering::Relaxed);
        words[header::MAX_JOINTS].store(MAX_JOINTS as u32, Ordering::Relaxed);
        words[header::TIME_BASE].store(time.code(), Ordering::Relaxed);
        words[header::ABI_VERSION].store(ABI_VERSION, Ordering::Relaxed);
        // The magic goes last and with a release, so a reader either sees a complete header or does
        // not recognise the mapping at all. A half-written header with valid magic is the one
        // opening failure a consumer could not detect.
        words[header::MAGIC_1].store(MAGIC_1, Ordering::Relaxed);
        words[header::MAGIC_0].store(MAGIC_0, Ordering::Release);
        Ok(Ring {
            words,
            slot_count,
            time,
        })
    }

    /// Open a ring somebody else created, validating the header.
    pub fn open(words: &'a [AtomicU32]) -> Result<Ring<'a>, ObsError> {
        if words.len() < HEADER_WORDS {
            return Err(ObsError::Geometry);
        }
        if words[header::MAGIC_0].load(Ordering::Acquire) != MAGIC_0
            || words[header::MAGIC_1].load(Ordering::Relaxed) != MAGIC_1
        {
            return Err(ObsError::NotARing);
        }
        if words[header::ABI_VERSION].load(Ordering::Relaxed) != ABI_VERSION {
            return Err(ObsError::Version);
        }
        if words[header::SLOT_WORDS].load(Ordering::Relaxed) as usize != SLOT_WORDS
            || words[header::MAX_JOINTS].load(Ordering::Relaxed) as usize != MAX_JOINTS
        {
            return Err(ObsError::Geometry);
        }
        let slot_count = words[header::SLOT_COUNT].load(Ordering::Relaxed) as usize;
        if slot_count == 0 || !slot_count.is_power_of_two() || words.len() < words_for(slot_count) {
            return Err(ObsError::Geometry);
        }
        let Some(time) = TimeBase::from_code(words[header::TIME_BASE].load(Ordering::Relaxed))
        else {
            return Err(ObsError::TimeBase);
        };
        Ok(Ring {
            words,
            slot_count,
            time,
        })
    }

    /// How many slots it holds.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// Which clock every stamp in this ring is on.
    pub fn time_base(&self) -> TimeBase {
        self.time
    }

    /// The stamp of the newest published observation, or `None` when nothing has been published or
    /// every attempt was torn by a write.
    ///
    /// The whole of what a process on the far side of a [`TimeBase::Simulated`] ring can know about
    /// the kernel's time: the kernel's `now` on a simulated body *is* the stamp of the state it read
    /// (§35.3), and the kernel publishes that state every cycle, including the cycles its read
    /// failed on. Read under the slot's seqlock like any other field, so a stamp half-written by the
    /// producer is retried rather than returned.
    pub fn newest_stamp(&self) -> Option<u64> {
        for _ in 0..READ_ATTEMPTS {
            let published = self.published();
            let index = (published.wrapping_sub(1) as usize) & (self.slot_count - 1);
            let slot = self.slot_at(index);
            let before = slot[slot::SEQLOCK].load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            if before & 1 == 1 {
                continue;
            }
            let stamp = (slot[slot::STAMP_LO].load(Ordering::Relaxed) as u64)
                | ((slot[slot::STAMP_HI].load(Ordering::Relaxed) as u64) << 32);
            core::sync::atomic::fence(Ordering::Acquire);
            if slot[slot::SEQLOCK].load(Ordering::Relaxed) == before {
                return Some(stamp);
            }
        }
        None
    }

    /// How many observations have been published, modulo 2^32.
    pub fn published(&self) -> u32 {
        self.words[header::PUBLISHED].load(Ordering::Acquire)
    }

    /// The writing end.
    pub fn producer(self) -> Producer<'a> {
        Producer { ring: self }
    }

    /// The reading end.
    pub fn consumer(self) -> Consumer<'a> {
        Consumer { ring: self }
    }

    fn slot_at(&self, index: usize) -> &[AtomicU32] {
        let start = HEADER_WORDS + index * SLOT_WORDS;
        &self.words[start..start + SLOT_WORDS]
    }
}

/// The one writer: the process that holds the RHAL.
#[derive(Debug)]
pub struct Producer<'a> {
    ring: Ring<'a>,
}

impl Producer<'_> {
    /// Publish one observation. Wait-free.
    ///
    /// The seqlock is `skynet-ring`'s, word for word and for the same reasons: odd while writing,
    /// even when complete, never back to zero, and the `published` counter last. A reader that
    /// arrives mid-write sees an odd counter; one that is lapped sees a changed counter; neither
    /// uses what it read.
    pub fn publish(&mut self, observation: &Observation) -> Result<(), ObsError> {
        if observation.joints > MAX_JOINTS {
            return Err(ObsError::TooManyJoints);
        }

        let published = self.ring.published();
        let index = (published as usize) & (self.ring.slot_count - 1);
        let slot = self.ring.slot_at(index);

        let counter = slot[slot::SEQLOCK].load(Ordering::Relaxed);
        let writing = counter.wrapping_add(1) | 1;
        slot[slot::SEQLOCK].store(writing, Ordering::Relaxed);
        core::sync::atomic::fence(Ordering::Release);

        slot[slot::STAMP_LO].store(observation.stamp_ns as u32, Ordering::Relaxed);
        slot[slot::STAMP_HI].store((observation.stamp_ns >> 32) as u32, Ordering::Relaxed);
        slot[slot::SEQ_LO].store(observation.seq as u32, Ordering::Relaxed);
        slot[slot::SEQ_HI].store((observation.seq >> 32) as u32, Ordering::Relaxed);
        slot[slot::JOINTS].store(observation.joints as u32, Ordering::Relaxed);
        slot[slot::FLAGS].store(observation.flags, Ordering::Relaxed);
        slot[slot::FAULT_CODE].store(observation.fault_code, Ordering::Relaxed);

        for (index, value) in observation.base_pose.iter().enumerate() {
            slot[slot::BASE_POSE + index].store(value.to_bits(), Ordering::Relaxed);
        }
        for (index, value) in observation.base_twist.iter().enumerate() {
            slot[slot::BASE_TWIST + index].store(value.to_bits(), Ordering::Relaxed);
        }

        // The whole fixed arrays, not only the declared joints — `skynet-ring`'s rule, and the
        // failure it prevents is the same one read the other way round: a shorter observation must
        // not leave the previous one's joint positions above it, because a stale position under a
        // fresh stamp is exactly what a policy would integrate into a command.
        for joint in 0..MAX_JOINTS {
            let inside = joint < observation.joints;
            let position = if inside {
                observation.position[joint]
            } else {
                // `NaN`, not zero. Beyond the declared joint count there is no measurement, and a
                // zero would be a claim that the joint is at its origin.
                f32::NAN
            };
            let velocity = if inside {
                observation.velocity[joint]
            } else {
                f32::NAN
            };
            let effort = if inside {
                observation.effort[joint]
            } else {
                f32::NAN
            };
            slot[slot::POSITION + joint].store(position.to_bits(), Ordering::Relaxed);
            slot[VELOCITY_AT + joint].store(velocity.to_bits(), Ordering::Relaxed);
            slot[EFFORT_AT + joint].store(effort.to_bits(), Ordering::Relaxed);
        }

        core::sync::atomic::fence(Ordering::Release);
        let complete = match writing.wrapping_add(1) {
            0 => 2,
            other => other,
        };
        slot[slot::SEQLOCK].store(complete, Ordering::Release);
        self.ring.words[header::PUBLISHED].store(published.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

/// How many times a read will retry before reporting [`Reading::Stale`].
const READ_ATTEMPTS: usize = 4;

/// The one reader: the policy runtime.
#[derive(Debug)]
pub struct Consumer<'a> {
    ring: Ring<'a>,
}

impl Consumer<'_> {
    /// Read the most recently published observation.
    ///
    /// `into` is left **untouched** unless the result is [`Reading::Fresh`], so a policy that reads
    /// a `Stale` or an `Empty` still holds whatever it had — and can tell that it is old, because
    /// the stamp it carries has not moved.
    pub fn read(&mut self, into: &mut Observation) -> Reading {
        for _ in 0..READ_ATTEMPTS {
            let published = self.ring.published();
            let index = (published.wrapping_sub(1) as usize) & (self.ring.slot_count - 1);
            let slot = self.ring.slot_at(index);

            let before = slot[slot::SEQLOCK].load(Ordering::Acquire);
            // Emptiness is asked of the slot rather than the counter, for the reason `skynet-ring`
            // gives: `published` wraps, so zero means "nothing yet" once and "four billion" for
            // ever after, and a slot's seqlock is zero only if nothing was ever written into it.
            if before == 0 {
                return Reading::Empty;
            }
            if before & 1 == 1 {
                continue;
            }

            let joints = slot[slot::JOINTS].load(Ordering::Relaxed) as usize;
            // Above the ceiling is a torn read or a corrupt slot; either way it is not an
            // observation, and clamping it would make plausible body state out of nonsense.
            if joints > MAX_JOINTS {
                continue;
            }

            let mut candidate = Observation {
                stamp_ns: (slot[slot::STAMP_LO].load(Ordering::Relaxed) as u64)
                    | ((slot[slot::STAMP_HI].load(Ordering::Relaxed) as u64) << 32),
                seq: (slot[slot::SEQ_LO].load(Ordering::Relaxed) as u64)
                    | ((slot[slot::SEQ_HI].load(Ordering::Relaxed) as u64) << 32),
                joints,
                flags: slot[slot::FLAGS].load(Ordering::Relaxed),
                fault_code: slot[slot::FAULT_CODE].load(Ordering::Relaxed),
                ..Observation::default()
            };
            for index in 0..7 {
                candidate.base_pose[index] =
                    f32::from_bits(slot[slot::BASE_POSE + index].load(Ordering::Relaxed));
            }
            for index in 0..6 {
                candidate.base_twist[index] =
                    f32::from_bits(slot[slot::BASE_TWIST + index].load(Ordering::Relaxed));
            }
            for joint in 0..joints {
                candidate.position[joint] =
                    f32::from_bits(slot[slot::POSITION + joint].load(Ordering::Relaxed));
                candidate.velocity[joint] =
                    f32::from_bits(slot[VELOCITY_AT + joint].load(Ordering::Relaxed));
                candidate.effort[joint] =
                    f32::from_bits(slot[EFFORT_AT + joint].load(Ordering::Relaxed));
            }

            core::sync::atomic::fence(Ordering::Acquire);
            if slot[slot::SEQLOCK].load(Ordering::Relaxed) != before {
                continue;
            }

            *into = candidate;
            return Reading::Fresh;
        }
        Reading::Stale
    }
}

#[cfg(test)]
extern crate alloc;

#[cfg(test)]
mod tests;
