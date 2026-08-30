//! The SPSC command ring (§25).
//!
//! ```text
//! Policy backend                       Safety Kernel                     RHAL
//! ──────────────                       ─────────────                     ────
//! desired[]  ──▶ ┌──────────────────┐
//!                │ SPSC ring, shm   │ ──▶ 8 guards ──▶ rhal_command ──▶ write_command()
//!                │ fixed 4 KiB slots│      (§7.1)
//!                └──────────────────┘
//! ```
//!
//! Single-producer / single-consumer, lock-free, fixed-size slots. No serialisation, no sockets, no
//! DDS, no allocation on the hot path — this crate is `no_std` and does not link `alloc`, so "the
//! command path never allocates" is a property of the build rather than a promise in a review.
//!
//! # Two properties §25 calls load-bearing
//!
//! **The ring is the only channel from the policy to the kernel.** Not a convention — a consequence
//! of the sandbox, which holds no actuator descriptor and no other writable mapping. This crate
//! cannot enforce that and does not pretend to; what it can do is be small enough that the claim is
//! checkable, which is why it holds a protocol and no I/O at all.
//!
//! **The kernel never blocks on the producer.** [`Consumer::read`] returns
//! [`Reading::Stale`] rather than waiting, and the caller runs its cycle on what it already has.
//! §7.1's `watchdog` guard then observes a sequence that has not advanced and the robot stops in a
//! controlled way. A kernel that waited for a policy would be a kernel whose real-time guarantee is
//! the policy's tail latency — so *nothing in this file can block*, and there is no method that
//! could.
//!
//! # Latest wins, and it is not a queue
//!
//! §47.4 makes the distinction the design turns on: a policy wants the freshest frame and an old one
//! is worthless, while a recorder wants every frame and completeness is the guarantee. **"A queue in
//! front of a policy is latency wearing the costume of reliability."** The same is true one step
//! later, of the commands a policy produces: a kernel draining a backlog would be executing a
//! trajectory the policy planned for a robot that has since moved.
//!
//! So the consumer reads **the most recently published slot** and never the oldest unread one. The
//! several slots exist only so a producer publishing at full rate cannot overwrite the slot a
//! consumer is mid-read of; they are not a buffer of pending work.
//!
//! # Why there is no `unsafe` in here
//!
//! Every other crate in this workspace carries `#![forbid(unsafe_code)]`, and a lock-free structure
//! shared between two processes is the obvious place for that streak to end. It does not end here.
//!
//! The shared region is modelled as `&[AtomicU32]` — a slice of atomics, which has interior
//! mutability, so two holders of the same slice may read and write it concurrently with no `unsafe`
//! and with the memory model spelling out exactly what each side observes. What genuinely does
//! require `unsafe` is turning an `mmap` of a `memfd` into that slice, which is one call in one
//! function, and it lives in the mapping layer rather than in the protocol. The protocol — the
//! seqlock, the wrap, the tear detection, the layout — is the part where a mistake is a moving
//! robot, and it is the part that is checked by the compiler.
//!
//! It also makes the whole thing testable on any platform: the suite below backs a ring with an
//! ordinary array, and it is exercising the identical arithmetic a shared mapping would.
//!
//! # The layout is a format
//!
//! Two processes read these bytes, and on a robot running a Python policy runtime against a Rust
//! kernel host they are two *editions*. §31.2's rule then applies — "a format with two
//! implementations of its own definition is not a format" — so the layout is written out in full
//! below rather than left implied by the code that happens to produce it.
//!
//! Every field is little-endian, which is not a neutral choice and is the right one: every target
//! this operating system runs on is little-endian, and a byte order that is converted on a path
//! measured in microseconds should be converted nowhere. A big-endian port would byte-swap here and
//! nowhere else.
//!
//! ```text
//! ── the header, 4096 bytes ────────────────────────────────────────────────
//! word 0     magic      "SKYR"
//! word 1     magic      "ING1"
//! word 2     abi_version
//! word 3     slot_words
//! word 4     slot_count      a power of two
//! word 5     max_channels
//! words 6-15 reserved, zero
//! word 16    published       count of slots published, wrapping
//! rest       reserved, zero
//!
//! ── each slot, 4096 bytes ─────────────────────────────────────────────────
//! word 0     seqlock         odd while being written, even when complete
//! word 1     reserved, zero
//! words 2-3  stamp_ns        when the command was produced, not when it was sent
//! words 4-5  seq             MUST advance; a repeat is a stalled producer
//! word 6     mode            0 position, 1 velocity, 2 torque
//! word 7     channels        how many of the arrays below carry meaning
//! words 8+   values          f32 bits, one per channel
//! then       mask            one byte per channel, four to a word; 0 = hold this joint
//! rest       reserved, zero
//! ```
//!
//! The `mask` is the field worth explaining, and §24 explains it: a `drop` binding — a channel the
//! policy emits that this body has no equivalent for — becomes a zero in the mask. Without it a
//! dropped channel would have to be encoded as some value, **and every value is a command**. The
//! mask is how the OS says *nothing* rather than saying *zero*, which on a torque-controlled joint
//! is the difference between holding position and going limp.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use core::sync::atomic::{AtomicU32, Ordering};

/// The version of this layout. Bumped only for a breaking change.
///
/// A producer and a consumer that disagree here do not negotiate: [`Ring::open`] refuses, for the
/// same reason §33 refuses a RHAL driver whose major does not match. Half a shared memory protocol
/// is worse than none, and the failure it produces is a command array read at the wrong offset.
pub const ABI_VERSION: u32 = 1;

/// `"SKYR"`, little-endian.
const MAGIC_0: u32 = u32::from_le_bytes(*b"SKYR");
/// `"ING1"`, little-endian.
const MAGIC_1: u32 = u32::from_le_bytes(*b"ING1");

/// Bytes in a slot, and in the header.
///
/// 4 KiB, from §25, and it is a page. A slot that straddled a page boundary would fault twice on
/// first touch and, on a mapping shared with another process, would put two slots on one page —
/// which is how a producer writing slot *n* comes to dirty the line a consumer is reading slot
/// *n − 1* from.
pub const SLOT_BYTES: usize = 4096;

/// `u32` words in a slot.
pub const SLOT_WORDS: usize = SLOT_BYTES / 4;

/// Bytes of header before the first slot. One page, for the reason above.
pub const HEADER_BYTES: usize = 4096;

/// `u32` words in the header.
pub const HEADER_WORDS: usize = HEADER_BYTES / 4;

/// The largest command this ring can carry.
///
/// Deliberately equal to `skynet_kernel::MAX_CHANNELS` and deliberately not imported from it: this
/// crate takes no dependency on the kernel, and a ring is a place two processes meet rather than a
/// place the kernel's opinions apply. [`tests::the_channel_ceiling_matches_the_kernels`] is what
/// keeps the two numbers equal — a comment saying they should be would not.
pub const MAX_CHANNELS: usize = 128;

// Header word offsets. Named rather than written out at each use, for the reason
// `skynet-blackbox`'s ring says: two lists of numbers that have to agree is one list too many, and
// the bug it produces is a ring that opens, passes every check, and reads a field four bytes along.
mod header {
    pub const MAGIC_0: usize = 0;
    pub const MAGIC_1: usize = 1;
    pub const ABI_VERSION: usize = 2;
    pub const SLOT_WORDS: usize = 3;
    pub const SLOT_COUNT: usize = 4;
    pub const MAX_CHANNELS: usize = 5;
    /// On its own cache line, 64 bytes in.
    ///
    /// The producer stores this every cycle and the consumer loads the geometry above it on every
    /// read; sharing a line would have a 1 kHz write invalidating a line that is only ever read,
    /// for nothing. It is the one field in the header that moves.
    pub const PUBLISHED: usize = 16;
}

// Slot word offsets.
mod slot {
    pub const SEQLOCK: usize = 0;
    pub const STAMP_LO: usize = 2;
    pub const STAMP_HI: usize = 3;
    pub const SEQ_LO: usize = 4;
    pub const SEQ_HI: usize = 5;
    pub const MODE: usize = 6;
    pub const CHANNELS: usize = 7;
    pub const VALUES: usize = 8;
}

/// Where the mask begins, in words: after the fixed value array.
const MASK_WORDS_AT: usize = slot::VALUES + MAX_CHANNELS;
/// Words the mask occupies: one byte per channel, four to a word.
const MASK_WORDS: usize = MAX_CHANNELS / 4;

/// The last word a slot uses. Everything above it is reserved and written zero.
const SLOT_USED_WORDS: usize = MASK_WORDS_AT + MASK_WORDS;

/// A slot must fit in its page, and this is the compiler's business rather than a test's.
///
/// The three constants that decide it — [`MAX_CHANNELS`], [`SLOT_BYTES`] and the mask packing — are
/// each plausible to change on their own, and the failure of getting it wrong is a `publish` that
/// writes past the end of one slot and into the head of the next. That is a torn command on a
/// neighbouring slot with a perfectly valid seqlock, which is the one corruption this format's
/// reader cannot see. It costs nothing to make it a build error.
const _: () = assert!(
    SLOT_USED_WORDS <= SLOT_WORDS,
    "a slot's fields do not fit in SLOT_BYTES"
);

/// Control modes, mirroring `SKR_MODE_*` in `bindings/c/skynet_rhal.h`.
pub mod mode {
    #![allow(missing_docs)]
    /// Radians, or metres for a prismatic joint.
    pub const POSITION: u32 = 0;
    /// rad/s, or m/s.
    pub const VELOCITY: u32 = 1;
    /// Newton-metres, or newtons.
    pub const TORQUE: u32 = 2;
}

/// Why a ring could not be created or opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingError {
    /// The region is too small for the header and the slots it claims.
    TooSmall,
    /// The first two words are not this format's.
    NotARing,
    /// The header's `abi_version` is not [`ABI_VERSION`].
    ///
    /// Carries what was found, because "this mapping is from an older OS image" and "this mapping
    /// is not a ring" are different investigations.
    AbiMismatch(u32),
    /// The slot count is zero, not a power of two, or the geometry disagrees with this build.
    Geometry,
    /// More channels were asked for than [`MAX_CHANNELS`].
    TooManyChannels,
}

impl RingError {
    /// A code for a log or a console.
    pub fn as_str(self) -> &'static str {
        match self {
            RingError::TooSmall => "too_small",
            RingError::NotARing => "not_a_ring",
            RingError::AbiMismatch(_) => "abi_mismatch",
            RingError::Geometry => "geometry",
            RingError::TooManyChannels => "too_many_channels",
        }
    }
}

/// One command, as it crosses the ring.
///
/// Fixed-size and `Copy`: a producer fills one on its stack, publishes it, and reuses it next cycle.
/// §24's rule that "every buffer is caller-allocated and reused across cycles, so the hot path never
/// touches an allocator" applies to this side of the RHAL as much as to the far side of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    /// When the policy produced this command, **not** when it was sent (§24).
    ///
    /// A stamp applied at send folds the producer's own scheduling delay into the number the
    /// watchdog and every latency budget downstream are measured against.
    pub stamp_ns: u64,
    /// The command sequence. **Must advance.**
    ///
    /// §24: "the watchdog treats a repeat as a stalled producer". This crate does not check it —
    /// checking is §7.1's `watchdog` guard, which is in the kernel where the response lives — but
    /// the field is carried faithfully so that the guard has something true to look at.
    pub seq: u64,
    /// One of [`mode`].
    pub mode: u32,
    /// How many entries of [`Self::values`] and [`Self::mask`] carry meaning.
    pub channels: usize,
    /// The commanded values, in the units [`Self::mode`] implies.
    pub values: [f32; MAX_CHANNELS],
    /// Per channel: zero means **hold this joint at its current setpoint** (§24).
    pub mask: [u8; MAX_CHANNELS],
}

impl Default for Command {
    fn default() -> Self {
        Command {
            stamp_ns: 0,
            seq: 0,
            mode: mode::POSITION,
            channels: 0,
            values: [0.0; MAX_CHANNELS],
            // Zero, which means *hold*. The default command is the one that asks for nothing, and
            // a `Command::default()` that quietly commanded every joint to its zero position would
            // be the worst default this crate could pick.
            mask: [0; MAX_CHANNELS],
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
    /// Distinct from [`Reading::Stale`] because it is a different moment: this is a robot whose
    /// policy has not produced its first command, which is normal at start and is not a fault.
    Empty,
    /// The producer overwrote the slot mid-read, repeatedly.
    ///
    /// Not an error and not a retry loop that could spin: [`Consumer::read`] gives up after a fixed
    /// number of attempts and says so, and the caller runs its cycle on the command it already
    /// holds. §25: the kernel never blocks on the producer.
    ///
    /// Seeing this at all means the producer is publishing faster than the consumer can copy a
    /// slot, sustained across every attempt — worth a counter and worth a look, and never worth a
    /// wait.
    Stale,
}

/// A ring, over a shared region of `u32` words.
///
/// Holds no buffer and no file. `skynetd`'s runtime backs it with a `memfd` mapping and the suite
/// backs it with an array, and both are exercising the same protocol — the same split, and for the
/// same reason, as `skynet-blackbox`'s ring.
#[derive(Debug, Clone, Copy)]
pub struct Ring<'a> {
    words: &'a [AtomicU32],
    slot_count: usize,
}

/// How many words a region must hold for a ring of `slot_count` slots.
pub const fn words_for(slot_count: usize) -> usize {
    HEADER_WORDS + slot_count * SLOT_WORDS
}

impl<'a> Ring<'a> {
    /// Write a fresh header over `words` and return the ring it describes.
    ///
    /// The producer's side of setup, performed once before either end runs. `slot_count` must be a
    /// power of two so the consumer's index arithmetic is a mask rather than a division — and, more
    /// to the point, so that the wrap of the `published` counter lands on a slot boundary. With a
    /// non-power-of-two count, the slot chosen either side of a `u32` wrap would jump, and the
    /// consumer would read a slot the producer was not writing on the one cycle in four billion
    /// that nobody tests.
    pub fn create(words: &'a [AtomicU32], slot_count: usize) -> Result<Ring<'a>, RingError> {
        if slot_count == 0 || !slot_count.is_power_of_two() {
            return Err(RingError::Geometry);
        }
        if words.len() < words_for(slot_count) {
            return Err(RingError::TooSmall);
        }

        // Zero everything first. A region handed over by an allocator, or a `memfd` reused after an
        // OTA, may hold anything at all, and a slot with a stale even seqlock and a plausible
        // sequence number is a command the consumer would accept.
        for word in words.iter() {
            word.store(0, Ordering::Relaxed);
        }

        words[header::SLOT_WORDS].store(SLOT_WORDS as u32, Ordering::Relaxed);
        words[header::SLOT_COUNT].store(slot_count as u32, Ordering::Relaxed);
        words[header::MAX_CHANNELS].store(MAX_CHANNELS as u32, Ordering::Relaxed);
        words[header::ABI_VERSION].store(ABI_VERSION, Ordering::Relaxed);

        // The magic last, and with a release. Everything above must be visible to any process that
        // sees the magic, and a consumer that opened a half-written header would read a slot count
        // of zero and conclude the ring was malformed — which it would be, for a microsecond.
        words[header::MAGIC_1].store(MAGIC_1, Ordering::Relaxed);
        words[header::MAGIC_0].store(MAGIC_0, Ordering::Release);

        Ok(Ring { words, slot_count })
    }

    /// Open a region another process created.
    pub fn open(words: &'a [AtomicU32]) -> Result<Ring<'a>, RingError> {
        if words.len() < HEADER_WORDS {
            return Err(RingError::TooSmall);
        }
        if words[header::MAGIC_0].load(Ordering::Acquire) != MAGIC_0
            || words[header::MAGIC_1].load(Ordering::Relaxed) != MAGIC_1
        {
            return Err(RingError::NotARing);
        }
        let abi = words[header::ABI_VERSION].load(Ordering::Relaxed);
        if abi != ABI_VERSION {
            return Err(RingError::AbiMismatch(abi));
        }
        let slot_words = words[header::SLOT_WORDS].load(Ordering::Relaxed) as usize;
        let max_channels = words[header::MAX_CHANNELS].load(Ordering::Relaxed) as usize;
        let slot_count = words[header::SLOT_COUNT].load(Ordering::Relaxed) as usize;
        // The geometry is checked rather than adopted. A ring claiming 8 KiB slots is a ring this
        // build cannot address, and reading it with this build's constants would silently take
        // every field from the wrong offset — which is the one failure mode a magic number and a
        // version cannot catch on their own.
        if slot_words != SLOT_WORDS
            || max_channels != MAX_CHANNELS
            || slot_count == 0
            || !slot_count.is_power_of_two()
        {
            return Err(RingError::Geometry);
        }
        if words.len() < words_for(slot_count) {
            return Err(RingError::TooSmall);
        }
        Ok(Ring { words, slot_count })
    }

    /// How many slots this ring holds.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// How many slots have been published, wrapping.
    ///
    /// Exposed for a console and for the tests. A caller reading this to decide whether to read a
    /// slot is doing the consumer's job worse: [`Consumer::read`] answers the same question and
    /// copes with the producer moving underneath it.
    pub fn published(&self) -> u32 {
        self.words[header::PUBLISHED].load(Ordering::Acquire)
    }

    /// Take the single producer.
    ///
    /// Consumed by value, so a second one cannot be made from the same `Ring` without another
    /// `open` — which is as close as a type system gets to enforcing "single producer" across two
    /// processes. Two writers on this protocol do not corrupt a slot dramatically; they interleave
    /// two commands into one, which is §25.2's *"a trajectory neither was trained on and neither
    /// can be held responsible for"* arriving one layer lower down.
    pub fn producer(self) -> Producer<'a> {
        Producer { ring: self }
    }

    /// Take the single consumer.
    pub fn consumer(self) -> Consumer<'a> {
        Consumer { ring: self }
    }

    fn slot_at(&self, index: usize) -> &'a [AtomicU32] {
        let start = HEADER_WORDS + index * SLOT_WORDS;
        &self.words[start..start + SLOT_WORDS]
    }
}

/// The one writer (§25).
#[derive(Debug)]
pub struct Producer<'a> {
    ring: Ring<'a>,
}

impl Producer<'_> {
    /// Publish a command. Wait-free: it takes a bounded number of stores and cannot block.
    ///
    /// # The seqlock, and why the odd number matters
    ///
    /// The slot's first word is incremented to an **odd** value before the payload is written and
    /// to the next **even** value after. A consumer reads the counter, the payload, then the
    /// counter again, and accepts the payload only if the counter was even and did not change. So a
    /// reader that arrives mid-write sees an odd counter and does not use what it read, and one
    /// that is lapped during the copy sees a changed counter and does not use what it read.
    ///
    /// The producer never waits for the reader, which is the property that makes this the right
    /// structure here rather than a queue: a policy stalling does not stall the kernel, and a kernel
    /// taking a long cycle does not stall the policy. What either of those *does* cause is visible
    /// — a sequence that stops advancing, or a [`Reading::Stale`] — and visible is the whole
    /// requirement.
    pub fn publish(&mut self, command: &Command) -> Result<(), RingError> {
        if command.channels > MAX_CHANNELS {
            return Err(RingError::TooManyChannels);
        }

        let published = self.ring.published();
        let index = (published as usize) & (self.ring.slot_count - 1);
        let slot = self.ring.slot_at(index);

        let counter = slot[slot::SEQLOCK].load(Ordering::Relaxed);
        // To the next odd value. The counter is per slot and wraps at 2^32, which at one publish
        // per slot per millisecond is a little over sixty-eight years on the smallest useful ring —
        // and a wrap is only ever a missed tear detection on the single read that straddles it.
        let writing = counter.wrapping_add(1) | 1;
        slot[slot::SEQLOCK].store(writing, Ordering::Relaxed);
        // Nothing below may be reordered above the odd store, or a reader could see new payload
        // under an even counter.
        core::sync::atomic::fence(Ordering::Release);

        slot[slot::STAMP_LO].store(command.stamp_ns as u32, Ordering::Relaxed);
        slot[slot::STAMP_HI].store((command.stamp_ns >> 32) as u32, Ordering::Relaxed);
        slot[slot::SEQ_LO].store(command.seq as u32, Ordering::Relaxed);
        slot[slot::SEQ_HI].store((command.seq >> 32) as u32, Ordering::Relaxed);
        slot[slot::MODE].store(command.mode, Ordering::Relaxed);
        slot[slot::CHANNELS].store(command.channels as u32, Ordering::Relaxed);

        // The whole fixed array, not only the declared channels. A shorter command must not leave
        // the previous one's values above it: they would be read as this command's the moment a
        // consumer trusted `channels` less carefully than it should, and a stale torque under a
        // fresh stamp is the worst artifact this ring could hand anybody.
        for channel in 0..MAX_CHANNELS {
            let value = if channel < command.channels {
                command.values[channel]
            } else {
                0.0
            };
            slot[slot::VALUES + channel].store(value.to_bits(), Ordering::Relaxed);
        }
        for word in 0..MASK_WORDS {
            let mut packed = 0u32;
            for byte in 0..4 {
                let channel = word * 4 + byte;
                let mask = if channel < command.channels {
                    command.mask[channel]
                } else {
                    // Zero: hold. Beyond the declared channel count there is no command, and *hold*
                    // is what "no command" means on this wire.
                    0
                };
                packed |= (mask as u32) << (byte * 8);
            }
            slot[MASK_WORDS_AT + word].store(packed, Ordering::Relaxed);
        }

        // Everything above must be visible before the counter goes even.
        core::sync::atomic::fence(Ordering::Release);
        // Never back to zero. A zero seqlock is this format's evidence that a slot has *never been
        // written*, which is how [`Consumer::read`] tells an empty ring from one whose `published`
        // counter has wrapped — and a counter that came round to zero after 2^32 writes to one slot
        // would report a live command as an empty ring for exactly one cycle. That is five hundred
        // years away at a kilohertz and it costs one branch to make it never, which is the right
        // trade for an invariant the reader depends on.
        let complete = match writing.wrapping_add(1) {
            0 => 2,
            other => other,
        };
        slot[slot::SEQLOCK].store(complete, Ordering::Release);

        // And the slot must be complete before it can be selected. A consumer that saw the
        // incremented `published` while the counter was still odd would simply retry, so this is
        // belt to the seqlock's braces rather than the only guard — but it is the store that makes
        // the new command *the* command, and it is the last thing that happens.
        self.ring.words[header::PUBLISHED].store(published.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

/// The one reader (§25).
#[derive(Debug)]
pub struct Consumer<'a> {
    ring: Ring<'a>,
}

/// How many times a read will retry before reporting [`Reading::Stale`].
///
/// Four, and the number is a statement rather than a tuning knob. Each attempt fails only if the
/// producer published a whole command during the copy; four consecutive failures means the producer
/// is running at least four times faster than the consumer can copy 672 bytes, sustained. That is
/// not a race worth waiting out, it is a misconfigured rate, and §25's answer to it is to run the
/// cycle on what is already held and let the watchdog see a sequence that has not advanced.
const READ_ATTEMPTS: usize = 4;

impl Consumer<'_> {
    /// Read the most recently published command. Never blocks, never allocates, never spins
    /// unboundedly.
    ///
    /// `into` is left **untouched** unless the result is [`Reading::Fresh`], so the caller's
    /// previous command survives a `Stale` or an `Empty` — which is precisely what §25 asks the
    /// kernel to run its cycle on.
    pub fn read(&mut self, into: &mut Command) -> Reading {
        for _ in 0..READ_ATTEMPTS {
            let published = self.ring.published();
            let index = (published.wrapping_sub(1) as usize) & (self.ring.slot_count - 1);
            let slot = self.ring.slot_at(index);

            let before = slot[slot::SEQLOCK].load(Ordering::Acquire);
            // Emptiness is asked of the **slot**, not of the counter, and that is not a detail.
            // `published` is a wrapping `u32`, so zero means "nothing yet" on the first cycle and
            // "four billion commands" on a robot that has been up a few weeks — and a consumer that
            // read the counter would report the second as the first, once, silently, with the drives
            // live. A slot's seqlock is zero only if nothing has ever been written into it, which is
            // true before the first publish and false ever afterwards.
            if before == 0 {
                return Reading::Empty;
            }
            if before & 1 == 1 {
                // Mid-write. Try again rather than reading a half-command.
                continue;
            }

            let mut candidate = Command {
                stamp_ns: (slot[slot::STAMP_LO].load(Ordering::Relaxed) as u64)
                    | ((slot[slot::STAMP_HI].load(Ordering::Relaxed) as u64) << 32),
                seq: (slot[slot::SEQ_LO].load(Ordering::Relaxed) as u64)
                    | ((slot[slot::SEQ_HI].load(Ordering::Relaxed) as u64) << 32),
                mode: slot[slot::MODE].load(Ordering::Relaxed),
                channels: slot[slot::CHANNELS].load(Ordering::Relaxed) as usize,
                values: [0.0; MAX_CHANNELS],
                mask: [0; MAX_CHANNELS],
            };
            // A channel count above the ceiling is a torn read or a corrupt slot; either way it is
            // not a command. Clamping it would produce a plausible short command out of nonsense,
            // which is worse than the retry.
            if candidate.channels > MAX_CHANNELS {
                continue;
            }
            for channel in 0..candidate.channels {
                candidate.values[channel] =
                    f32::from_bits(slot[slot::VALUES + channel].load(Ordering::Relaxed));
            }
            for channel in 0..candidate.channels {
                let packed = slot[MASK_WORDS_AT + channel / 4].load(Ordering::Relaxed);
                candidate.mask[channel] = (packed >> ((channel % 4) * 8)) as u8;
            }

            // Nothing above may be reordered below this load, or the check would be of a counter
            // read before the payload it is meant to vouch for.
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
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// A region big enough for `n` slots, as an ordinary array of atomics.
    ///
    /// This is the whole of what a test needs, and it is the point of modelling the mapping as
    /// `&[AtomicU32]`: the protocol under test here is byte for byte the protocol that runs over a
    /// `memfd`, with nothing stubbed and nothing skipped.
    fn region(n: usize) -> Vec<AtomicU32> {
        (0..words_for(n)).map(|_| AtomicU32::new(0)).collect()
    }

    fn command(seq: u64, channels: usize) -> Command {
        let mut c = Command {
            seq,
            stamp_ns: seq * 1_000_000,
            mode: mode::TORQUE,
            channels,
            ..Command::default()
        };
        for channel in 0..channels {
            c.values[channel] = channel as f32 + 0.5;
            c.mask[channel] = 1;
        }
        c
    }

    #[test]
    fn the_channel_ceiling_matches_the_kernels() {
        // Written out rather than imported, because this crate takes no dependency on the kernel.
        // 128 is `skynet_kernel::MAX_CHANNELS`; if that moves, this fails and somebody decides
        // which of the two is wrong rather than discovering it through a truncated command.
        assert_eq!(MAX_CHANNELS, 128);
    }

    #[test]
    fn a_fresh_ring_has_nothing_in_it() {
        let words = region(4);
        let ring = Ring::create(&words, 4).expect("create");
        let mut into = Command::default();
        assert_eq!(ring.consumer().read(&mut into), Reading::Empty);
    }

    #[test]
    fn a_published_command_arrives_intact() {
        let words = region(4);
        let ring = Ring::create(&words, 4).expect("create");
        let mut producer = ring.producer();
        producer.publish(&command(7, 3)).expect("publish");

        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, 7);
        assert_eq!(got.stamp_ns, 7_000_000);
        assert_eq!(got.mode, mode::TORQUE);
        assert_eq!(got.channels, 3);
        assert_eq!(&got.values[..3], &[0.5, 1.5, 2.5]);
        assert_eq!(&got.mask[..3], &[1, 1, 1]);
    }

    #[test]
    fn a_zero_mask_survives_the_round_trip() {
        // The field §24 spends a paragraph on: the mask is how the OS says *nothing* rather than
        // saying zero, and a ring that dropped it would turn every `drop` binding into a command of
        // zero — which on a torque-controlled joint is the difference between holding position and
        // going limp. Zero is also the packing's default, so this pins that the byte arrives
        // because it was written and not because nothing overwrote it.
        let words = region(2);
        let ring = Ring::create(&words, 2).expect("create");
        let mut sent = command(1, 4);
        sent.mask = [0; MAX_CHANNELS];
        sent.mask[2] = 1;
        ring.producer().publish(&sent).expect("publish");

        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(&got.mask[..4], &[0, 0, 1, 0]);
        // And the values are still there under the zeroes, because a held joint is held at its own
        // setpoint rather than at whatever the policy happened to emit for it.
        assert_eq!(&got.values[..4], &[0.5, 1.5, 2.5, 3.5]);
    }

    #[test]
    fn every_channel_of_a_full_command_crosses() {
        // The ceiling exactly, so an off-by-one in the mask packing shows up here rather than on a
        // whole-body humanoid.
        let words = region(1);
        let ring = Ring::create(&words, 1).expect("create");
        let mut sent = command(3, MAX_CHANNELS);
        for channel in 0..MAX_CHANNELS {
            sent.mask[channel] = (channel % 251) as u8;
        }
        ring.producer().publish(&sent).expect("publish");

        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(got.channels, MAX_CHANNELS);
        assert_eq!(got.values, sent.values);
        assert_eq!(got.mask, sent.mask);
    }

    #[test]
    fn the_latest_wins_rather_than_the_oldest_unread() {
        // Six commands into a four-slot ring, with nobody reading. A queue would hand back the
        // first; §47.4's rule is that an old frame is worthless, so this hands back the sixth.
        let words = region(4);
        let ring = Ring::create(&words, 4).expect("create");
        let mut producer = ring.producer();
        for seq in 1..=6 {
            producer.publish(&command(seq, 2)).expect("publish");
        }
        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, 6);
    }

    #[test]
    fn reading_twice_without_a_publish_returns_the_same_command() {
        // Not an error, and not empty. §25: with no fresh frame the kernel runs its cycle on what
        // it has, and §7.1's watchdog guard is what notices the sequence has not advanced — which
        // it can only do if the sequence it is handed is the truth rather than a placeholder.
        let words = region(2);
        let ring = Ring::create(&words, 2).expect("create");
        ring.producer().publish(&command(11, 1)).expect("publish");
        let mut consumer = ring.consumer();
        let mut got = Command::default();
        assert_eq!(consumer.read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, 11);
        assert_eq!(consumer.read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, 11);
    }

    #[test]
    fn a_shorter_command_does_not_inherit_the_previous_ones_values() {
        // The slot is reused, so the tail of a longer command sits in it. A consumer trusting
        // `channels` never looks there — and the values are cleared anyway, because "never looks"
        // is a property of today's consumer and the bytes outlive it.
        let words = region(1);
        let ring = Ring::create(&words, 1).expect("create");
        let mut producer = ring.producer();
        producer.publish(&command(1, 8)).expect("publish");
        producer.publish(&command(2, 2)).expect("publish");

        let values_at = HEADER_WORDS + slot::VALUES;
        for channel in 2..8 {
            assert_eq!(
                words[values_at + channel].load(Ordering::Relaxed),
                0,
                "channel {channel} kept a value from the longer command"
            );
        }
        let mask_at = HEADER_WORDS + MASK_WORDS_AT;
        // Channels 0 and 1 masked, 2 to 7 cleared: the low two bytes of the first word set, the
        // rest of it zero.
        assert_eq!(words[mask_at].load(Ordering::Relaxed) & 0xffff_0000, 0);
    }

    #[test]
    fn a_mid_write_slot_is_not_read() {
        // The seqlock's whole job, staged by hand: leave the counter odd and confirm the consumer
        // reports `Stale` rather than handing back the payload underneath it.
        let words = region(1);
        let ring = Ring::create(&words, 1).expect("create");
        ring.producer().publish(&command(4, 1)).expect("publish");

        let counter = HEADER_WORDS + slot::SEQLOCK;
        words[counter].store(
            words[counter].load(Ordering::Relaxed) | 1,
            Ordering::Relaxed,
        );

        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Stale);
        // Untouched, so the caller still holds the last command it accepted — which is what §25
        // asks the kernel to run its cycle on.
        assert_eq!(got, Command::default());
    }

    #[test]
    fn opening_something_that_is_not_a_ring_is_refused() {
        let words = region(2);
        assert_eq!(Ring::open(&words).unwrap_err(), RingError::NotARing);
    }

    #[test]
    fn a_ring_from_another_abi_is_refused_and_says_which() {
        let words = region(2);
        Ring::create(&words, 2).expect("create");
        words[header::ABI_VERSION].store(9, Ordering::Relaxed);
        // The version it found, not a bare refusal: "this mapping is from an older OS image" and
        // "this mapping is not a ring" are two different investigations.
        assert_eq!(Ring::open(&words).unwrap_err(), RingError::AbiMismatch(9));
    }

    #[test]
    fn a_ring_with_a_geometry_this_build_cannot_address_is_refused() {
        let words = region(2);
        Ring::create(&words, 2).expect("create");
        words[header::SLOT_WORDS].store(2048, Ordering::Relaxed);
        assert_eq!(Ring::open(&words).unwrap_err(), RingError::Geometry);
    }

    #[test]
    fn a_slot_count_that_is_not_a_power_of_two_is_refused() {
        let words = region(8);
        assert_eq!(Ring::create(&words, 3).unwrap_err(), RingError::Geometry);
    }

    #[test]
    fn a_region_too_small_for_its_own_claim_is_refused() {
        let words = region(2);
        assert_eq!(Ring::create(&words, 4).unwrap_err(), RingError::TooSmall);
    }

    #[test]
    fn more_channels_than_the_ring_carries_is_an_error_and_not_a_truncation() {
        let words = region(1);
        let ring = Ring::create(&words, 1).expect("create");
        let mut producer = ring.producer();
        let too_many = Command {
            channels: MAX_CHANNELS + 1,
            ..Command::default()
        };
        assert_eq!(
            producer.publish(&too_many).unwrap_err(),
            RingError::TooManyChannels
        );
        // And nothing was published, so a consumer sees the ring exactly as it was.
        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Empty);
    }

    #[test]
    fn the_published_counter_wraps_without_losing_the_command() {
        // Why `slot_count` must be a power of two, and why emptiness is asked of the slot rather
        // than of the counter. Driven to the top of a `u32` by hand and published across the wrap:
        // `published` comes back to zero, which on a naive reader is the spelling of "nothing has
        // ever been sent" — on a robot that has been up about seven weeks at a kilohertz, with the
        // drives live.
        let words = region(4);
        let ring = Ring::create(&words, 4).expect("create");
        words[header::PUBLISHED].store(u32::MAX, Ordering::Relaxed);
        ring.producer().publish(&command(42, 1)).expect("publish");
        assert_eq!(ring.published(), 0);

        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, 42);
    }

    #[test]
    fn a_seqlock_never_comes_back_to_zero() {
        // The invariant the test above depends on. Wound to the last even value before the wrap and
        // published once: the counter must step over zero rather than onto it, or a live slot would
        // read as one that had never been written.
        let words = region(1);
        let ring = Ring::create(&words, 1).expect("create");
        let counter = HEADER_WORDS + slot::SEQLOCK;
        words[counter].store(u32::MAX - 1, Ordering::Relaxed);
        ring.producer().publish(&command(1, 1)).expect("publish");
        assert_ne!(words[counter].load(Ordering::Relaxed), 0);
        assert_eq!(words[counter].load(Ordering::Relaxed) & 1, 0);

        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, 1);
    }

    /// A producer and a consumer on two real threads, which is the only way to test this.
    ///
    /// Every assertion above stages a race by hand, which proves the arithmetic and proves nothing
    /// about the memory ordering — the fences are exactly the part that a single-threaded test
    /// cannot see, because a single thread observes its own stores in program order whatever
    /// orderings are written.
    ///
    /// The invariant is **self-consistency**, not delivery: a slot may be missed, and missing one is
    /// the design working. What must never happen is a command whose fields came from two different
    /// publishes, so every value is derived from the sequence number and checked against it. A torn
    /// read is a command the policy never issued, arriving at a robot, with nothing downstream in a
    /// position to notice.
    #[test]
    fn a_reader_racing_a_writer_never_sees_a_torn_command() {
        use core::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::thread;

        const CYCLES: u64 = 20_000;
        // One slot, deliberately: the producer overwrites the only slot on every publish, so the
        // consumer is racing it on every read. With eight slots the window is so narrow the test
        // would pass without proving anything.
        let words: Arc<Vec<AtomicU32>> = Arc::new(region(1));
        Ring::create(&words, 1).expect("create");
        let done = Arc::new(AtomicBool::new(false));

        let writer_words = Arc::clone(&words);
        let writer_done = Arc::clone(&done);
        let writer = thread::spawn(move || {
            let ring = Ring::open(&writer_words).expect("open");
            let mut producer = ring.producer();
            for seq in 1..=CYCLES {
                let mut c = Command {
                    seq,
                    stamp_ns: seq.wrapping_mul(1_000_000),
                    mode: (seq % 3) as u32,
                    channels: MAX_CHANNELS,
                    ..Command::default()
                };
                for channel in 0..MAX_CHANNELS {
                    c.values[channel] = (seq as f32) + channel as f32 / 1024.0;
                    c.mask[channel] = (seq % 2) as u8;
                }
                producer.publish(&c).expect("publish");
            }
            writer_done.store(true, Ordering::Release);
        });

        let reader_words = Arc::clone(&words);
        let reader_done = Arc::clone(&done);
        let reader = thread::spawn(move || {
            let ring = Ring::open(&reader_words).expect("open");
            let mut consumer = ring.consumer();
            let mut got = Command::default();
            let mut fresh = 0u64;
            let mut stale = 0u64;
            loop {
                match consumer.read(&mut got) {
                    Reading::Fresh => {
                        fresh += 1;
                        assert_eq!(got.stamp_ns, got.seq.wrapping_mul(1_000_000), "stamp");
                        assert_eq!(got.mode, (got.seq % 3) as u32, "mode");
                        assert_eq!(got.channels, MAX_CHANNELS, "channels");
                        for channel in 0..MAX_CHANNELS {
                            assert_eq!(
                                got.values[channel],
                                (got.seq as f32) + channel as f32 / 1024.0,
                                "value {channel} of command {}",
                                got.seq
                            );
                            assert_eq!(got.mask[channel], (got.seq % 2) as u8, "mask {channel}");
                        }
                    }
                    Reading::Stale => stale += 1,
                    Reading::Empty => {}
                }
                if reader_done.load(Ordering::Acquire) {
                    break;
                }
            }
            (fresh, stale)
        });

        writer.join().expect("writer");
        let (fresh, _stale) = reader.join().expect("reader");
        // The ring must have actually been read, or the loop above asserted nothing. This is the
        // check that stops the test passing because the reader started after the writer finished.
        assert!(fresh > 0, "the reader never saw a command");

        // And the last command is there afterwards, whatever the race did in the middle.
        let ring = Ring::open(&words).expect("open");
        let mut got = Command::default();
        assert_eq!(ring.consumer().read(&mut got), Reading::Fresh);
        assert_eq!(got.seq, CYCLES);
    }
}
