#![forbid(unsafe_code)]
//! §47.6's mapping half — the pixels, and the loan that keeps a slot still while somebody reads it.
//!
//! §47.6 built the **bookkeeping** half in [`skynet_contracts::media`]: `(stream, slot, generation)`
//! handles, generation-guarded acquires, single-owner loans, drop-and-count on exhaustion, all
//! reproduced in three editions and pinned by ten corpus cases. It then said, in one sentence, what
//! was left:
//!
//! > This is the bookkeeping half. The mapping half — read-only mapping of the named slot in the
//! > policy's address space — is per-OS code; what it must honour is that the pool may reuse the
//! > slot the moment it is released.
//!
//! This crate is that, and **only** that. The rule stays where it is: which slot a publish takes,
//! which handle an acquire refuses and what each refusal is called are all
//! [`skynet_contracts::media::FramePool`]'s, and a second implementation of them here would be
//! §47.6a happening again — where a selection rule that looked equivalent cost the reader its frame
//! on nearly every cycle and six unit tests agreed it was fine.
//!
//! # Two mappings, and the asymmetry is the design
//!
//! §25's command ring is created by the runtime and written by the **policy**: the untrusted side
//! is the producer, so the runtime's mapping carries no `PROT_WRITE` and *the policy is the only
//! writer* is a property of the page tables.
//!
//! A frame pool is the other way round. Capture is the producer and the policy is the consumer, so
//! the pixels are mapped **read-only** into the policy and the same sentence protects the other
//! direction: a policy cannot scribble on a frame the black box is also encoding, cannot forge what
//! the guards' proximity reading was computed from, and cannot shrink the file under the writer
//! (`F_SEAL_SHRINK`, added before the descriptor leaves the creating process).
//!
//! But §47.6 requires that *"a slot still acquired by a reader is never reused under it"*, and the
//! writer is in a different process from the reader that acquired it. Something has to cross back.
//!
//! So there are **two** regions and they carry opposite permissions:
//!
//! | Region | Holds | The policy's access |
//! |---|---|---|
//! | **frames** | the header, the per-slot descriptors, and the pixels | **read-only** |
//! | **loans** | one `u64` per slot: the generation this reader claims, or 0 | **read-write** |
//!
//! The split is what lets both properties hold at once. The reader can say *I am holding slot 3
//! generation 9* without being able to touch a pixel, and the writer can honour that without
//! trusting the reader with anything else.
//!
//! # What a hostile policy can do with the loans region, and why it is survivable
//!
//! It can write nonsense: a slot index past the end, a generation that was never issued, or a claim
//! on every slot at once and never release one. Each is bounded by construction:
//!
//! * an index past the end is not addressable — the region is exactly `slots` words long and every
//!   read here is a `get`;
//! * a generation that does not match the slot's is not a loan, because [`Writer::publish`] compares
//!   it against what it issued rather than believing it;
//! * **claiming every slot is the interesting one**, and §47.6 already decided what happens: the
//!   frame is *dropped and counted*, and capture is never stalled (§47.4). A policy can therefore
//!   deny itself vision and can neither stop the camera, stall the encoder, nor delay a control
//!   cycle. The counter is the evidence, and it is in the stats §47.6 already specifies.
//!
//! That is the same posture `skynet-ring-shm` takes about a hostile producer: *"what it can still do
//! is put nonsense in a slot… all of it is a command, which is what the Safety Kernel is for."*
//! Here the worst it can do is starve itself, visibly.
//!
//! # The tear rule, which is the reader's and not the writer's
//!
//! A loan makes a slot **stable**, not atomic. A reader that has not taken one — one calling
//! [`Reader::peek`] to look at the newest frame without committing — can be overwritten mid-read,
//! so `peek` reads the generation, reads the bytes, and reads the generation again, refusing if it
//! moved. That is `skynet-ring`'s seqlock discipline with a different word in it, and it is why the
//! generation is stored per slot rather than only in the header.
//!
//! [`Reader::acquire`] does not need it: the loan is published before the bytes are read, and
//! `publish` will not take a slot under a live loan. The two paths are separate on purpose — a
//! policy that wants the freshest pixels with no commitment and a policy that wants a frame it can
//! work on for several milliseconds are different programs, and §47.4 already says the pipeline
//! serves two consumers with opposite disciplines.
//!
//! # Layout
//!
//! One frames region per pool, laid out so that a reader can address a slot with arithmetic and no
//! allocation:
//!
//! ```text
//! ┌──────────────── header, 64 bytes ─────────────────┐
//! │ magic  version  slots  slot_bytes  stream_len  …  │
//! ├──────────────── stream name, 64 bytes ────────────┤
//! ├──────────────── descriptors, 32 × slots ──────────┤
//! │ generation  sequence  stamp_ns  len  …            │
//! ├──────────────── pixels, slot_bytes × slots ───────┤
//! └───────────────────────────────────────────────────┘
//! ```
//!
//! The magic is **not** the command ring's and not the observation ring's, for the reason §25 gives
//! about the pair it already had: a consumer built for one format that opened another is refused at
//! the first word rather than reading somebody else's bytes as its own.

use core::sync::atomic::{AtomicU8, Ordering};

use skynet_contracts::media::{FrameHandle, FrameIdentity, FramePool, FrameRefusalCode};
use skynet_ring_shm::{Access, Mapping, ShmError};

/// `"SKYFRM01"` little-endian. Different from §25's ring and §25's observation ring, deliberately.
pub const MAGIC: u64 = 0x3130_4d52_4659_4b53;

/// The layout version. Bumped for a change a reader could not detect any other way.
pub const VERSION: u32 = 1;

/// Bytes of header before the stream name.
const HEADER_BYTES: usize = 64;

/// Bytes the stream name occupies, NUL-padded.
const NAME_BYTES: usize = 64;

/// Bytes per slot descriptor.
const DESCRIPTOR_BYTES: usize = 32;

/// The largest stream name this layout carries.
pub const MAX_NAME: usize = NAME_BYTES - 1;

/// Why a pool could not be created, mapped or read.
///
/// Distinct from [`FrameRefusal`], which is §47.6's vocabulary for *this handle is not one you may
/// use*. These are failures of the mapping rather than of the protocol, and conflating them would
/// put "the kernel would not give us memory" and "your handle is a generation stale" in one word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// The shared mapping could not be created, mapped or passed.
    Shm(ShmError),
    /// A capacity, slot size or stream name this layout cannot hold.
    Geometry(&'static str),
    /// The region does not begin with this layout's magic, or carries a version this build cannot
    /// read. Never a partial read: a pool that is not one is refused at the first word.
    NotAPool,
    /// §47.6 refused the operation, in the contract's own vocabulary.
    ///
    /// Carried through rather than restated. This crate has no opinion about what `pool_exhausted`
    /// is called, and a second spelling of that vocabulary is §78's shape — the thing this whole
    /// section exists downstream of.
    ///
    /// `pool_exhausted` in particular is **not** an error in the ordinary sense: §47.4 says capture
    /// is never stalled, so every slot being on loan means the frame is dropped and counted and the
    /// caller's correct response is to carry on with the next one.
    Refused(FrameRefusalCode),
    /// A frame larger than the slot it was published into.
    ///
    /// Refused rather than truncated, for §8's reason about half-parsed descriptors: a policy given
    /// nine tenths of an image has no way to know, and every downstream number computed from it
    /// would be wrong in a way that looks like a bad model rather than a bad pipeline.
    TooLarge {
        /// What the frame needed.
        needed: usize,
        /// What a slot holds.
        capacity: usize,
    },
}

impl core::fmt::Display for PoolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PoolError::Shm(error) => write!(f, "the frame pool's shared mapping failed: {error:?}"),
            PoolError::Refused(code) => write!(f, "§47.6 refused this frame: {}", code.as_str()),
            PoolError::Geometry(why) => write!(f, "this frame pool geometry is not usable: {why}"),
            PoolError::NotAPool => write!(
                f,
                "this mapping does not begin with a frame pool header. §47.6's pool, §25's command \
                 ring and §25's observation ring have different magic so that a consumer built for \
                 one is refused at the first word rather than reading the other's bytes as its own"
            ),
            PoolError::TooLarge { needed, capacity } => write!(
                f,
                "a frame of {needed} bytes does not fit a slot of {capacity}. The pool is refused \
                 the frame rather than given part of one: a policy handed nine tenths of an image \
                 has no way to know"
            ),
        }
    }
}

/// How the regions are sized, and what both ends must agree on.
///
/// Carried beside the descriptors when a pool is passed, rather than read out of the header by the
/// receiver, for the reason `skynet-ring-shm` gives about `bytes`: a receiver that mapped more than
/// the creator sized would take a `SIGBUS` on the first read past the end, and the size therefore
/// has to be known *before* the mapping rather than out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// How many slots. §47.5's declared capacity, never rounded.
    pub slots: usize,
    /// How many bytes one frame may occupy.
    pub slot_bytes: usize,
}

impl Geometry {
    /// Total bytes of the frames region.
    pub fn frame_bytes(&self) -> usize {
        HEADER_BYTES + NAME_BYTES + self.slots * DESCRIPTOR_BYTES + self.slots * self.slot_bytes
    }

    /// Total bytes of the loans region: one `u64` per slot.
    pub fn loan_bytes(&self) -> usize {
        self.slots * 8
    }

    fn check(&self) -> Result<(), PoolError> {
        if self.slots == 0 {
            return Err(PoolError::Geometry("a pool with no slots holds no frames"));
        }
        if self.slot_bytes == 0 {
            return Err(PoolError::Geometry("a slot of no bytes holds no pixels"));
        }
        Ok(())
    }
}

// ── Field offsets ────────────────────────────────────────────────────────────
//
// Named rather than inlined, because every one of them is read by two processes and a reader that
// disagreed with a writer about one would produce plausible garbage rather than an error.

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_SLOTS: usize = 12;
const OFF_SLOT_BYTES: usize = 16;
const OFF_LATEST: usize = 24;

/// Descriptor field offsets, within one 32-byte descriptor.
const D_GENERATION: usize = 0;
const D_SEQUENCE: usize = 8;
const D_STAMP: usize = 16;
const D_LEN: usize = 24;

fn read_u64(bytes: &[AtomicU8], at: usize) -> u64 {
    let mut value = [0u8; 8];
    for (index, slot) in value.iter_mut().enumerate() {
        *slot = bytes[at + index].load(Ordering::Relaxed);
    }
    u64::from_le_bytes(value)
}

fn write_u64(bytes: &[AtomicU8], at: usize, value: u64) {
    for (index, byte) in value.to_le_bytes().iter().enumerate() {
        bytes[at + index].store(*byte, Ordering::Relaxed);
    }
}

fn read_u32(bytes: &[AtomicU8], at: usize) -> u32 {
    let mut value = [0u8; 4];
    for (index, slot) in value.iter_mut().enumerate() {
        *slot = bytes[at + index].load(Ordering::Relaxed);
    }
    u32::from_le_bytes(value)
}

fn write_u32(bytes: &[AtomicU8], at: usize, value: u32) {
    for (index, byte) in value.to_le_bytes().iter().enumerate() {
        bytes[at + index].store(*byte, Ordering::Relaxed);
    }
}

/// Where one slot's descriptor begins.
fn descriptor_at(slot: usize) -> usize {
    HEADER_BYTES + NAME_BYTES + slot * DESCRIPTOR_BYTES
}

/// Where one slot's pixels begin.
fn pixels_at(geometry: &Geometry, slot: usize) -> usize {
    HEADER_BYTES + NAME_BYTES + geometry.slots * DESCRIPTOR_BYTES + slot * geometry.slot_bytes
}

/// The loans region, as one claim per slot.
///
/// A `u64` rather than a flag, and it holds the **generation** the reader claims rather than a
/// boolean. What that buys is narrower than it first looks, and the difference is worth stating
/// because the first draft of this comment claimed the wider version and a test disproved it.
///
/// * A claim naming a generation the slot **no longer holds** is not a loan. So a reader that
///   released a slot and then wrote its old claim back — or a claim left in the region from a
///   generation ago — holds nothing, and the writer is free to take the slot.
/// * A claim naming the generation the slot **does** hold is a loan, and it stays one until it is
///   released. A reader that died holding it pins that slot, and it pins it indefinitely: the
///   writer will not reuse the slot, so the generation never advances, so the claim never stops
///   matching. A boolean would behave identically here.
///
/// That is why [`Writer::revoke_loans`] exists rather than a timeout. The runtime knows when a
/// policy process is gone — it launched the unit — and a party that knows is a better source than
/// a deadline somebody has to tune. A timeout here would also have to be longer than the longest
/// legitimate hold, which §47.6 deliberately does not bound: a policy holding a frame for several
/// milliseconds is the case loans exist for.
struct Loans<'a> {
    words: &'a [AtomicU8],
    slots: usize,
}

impl<'a> Loans<'a> {
    fn of(mapping: &'a Mapping, slots: usize) -> Loans<'a> {
        Loans {
            words: mapping.bytes_slice(),
            slots,
        }
    }

    fn claim(&self, slot: usize) -> u64 {
        if slot >= self.slots {
            return 0;
        }
        read_u64(self.words, slot * 8)
    }

    fn set(&self, slot: usize, generation: u64) {
        if slot < self.slots {
            write_u64(self.words, slot * 8, generation);
        }
    }
}

/// The writing half: capture publishes frames into it.
///
/// Holds the authoritative [`FramePool`] — the contract's, so the selection rule, the generations
/// and the refusals are §47.6's rather than this file's.
pub struct Writer {
    frames: Mapping,
    loans: Mapping,
    geometry: Geometry,
    pool: FramePool,
    stream: String,
}

impl Writer {
    /// Create both regions and write the header.
    ///
    /// The pool is empty and the latest handle is absent, which reads as *no frame yet* rather than
    /// as a frame of zeros — §47.6's rule, and the reason the header carries a sentinel rather than
    /// slot 0.
    pub fn create(stream: &str, geometry: Geometry) -> Result<Writer, PoolError> {
        geometry.check()?;
        if stream.is_empty() || stream.len() > MAX_NAME {
            return Err(PoolError::Geometry(
                "a stream name must be between 1 and 63 bytes",
            ));
        }
        let pool = FramePool::new(stream, geometry.slots).ok_or(PoolError::Geometry(
            "the contract refused this stream and capacity (§47.5)",
        ))?;

        let frames = Mapping::create_bytes("skynet-frames", geometry.frame_bytes(), geometry.slots)
            .map_err(PoolError::Shm)?;
        let loans =
            Mapping::create_bytes("skynet-frame-loans", geometry.loan_bytes(), geometry.slots)
                .map_err(PoolError::Shm)?;

        let bytes = frames.bytes_slice();
        write_u64(bytes, OFF_MAGIC, MAGIC);
        write_u32(bytes, OFF_VERSION, VERSION);
        write_u32(bytes, OFF_SLOTS, geometry.slots as u32);
        write_u64(bytes, OFF_SLOT_BYTES, geometry.slot_bytes as u64);
        // `u64::MAX` is *no frame has been published*. Slot 0 would be indistinguishable from a
        // published first frame, which is the "frame of zeros" §47.6 refuses to produce.
        write_u64(bytes, OFF_LATEST, u64::MAX);
        for (index, byte) in stream.as_bytes().iter().enumerate() {
            bytes[HEADER_BYTES + index].store(*byte, Ordering::Relaxed);
        }

        Ok(Writer {
            frames,
            loans,
            geometry,
            pool,
            stream: stream.to_string(),
        })
    }

    /// The two descriptors to pass to the reader, frames first.
    ///
    /// Both or neither, exactly as §25's command and observation rings cross together: a policy
    /// handed pixels it cannot take a loan on would hold frames the writer is free to overwrite
    /// under it, which is the one property this crate exists to provide.
    #[cfg(target_os = "linux")]
    pub fn descriptors(&self) -> [core::ffi::c_int; 2] {
        [self.frames.fd(), self.loans.fd()]
    }

    /// The geometry a reader needs before it maps anything.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Drop every loan, because the reader that took them is gone.
    ///
    /// Called by the runtime when a policy's process exits, and by nothing else. A loan is released
    /// by the reader in the ordinary case; this is for the case where there is no reader left to
    /// release it, and without it a policy that crashed while holding a frame would cost the pool
    /// that slot for the life of the robot (see [`Loans`] for why the generation does not save it).
    ///
    /// It is deliberately **not** a timeout. §47.6 puts no bound on how long a policy may hold a
    /// frame — several milliseconds of inference is the case loans exist for — so any deadline
    /// short enough to reclaim a crashed reader's slot promptly is short enough to take a working
    /// reader's frame out from under it. The runtime launched the unit and knows when it is gone,
    /// which is a fact rather than an estimate.
    pub fn revoke_loans(&mut self) {
        let loans = Loans::of(&self.loans, self.geometry.slots);
        for slot in 0..self.geometry.slots {
            loans.set(slot, 0);
        }
        self.reconcile();
    }

    /// How the pool is doing — §47.6's stats, including the drop count.
    pub fn stats(&self) -> skynet_contracts::media::FramePoolStats {
        self.pool.stats()
    }

    /// Publish a frame, and return the handle a reader may take it by.
    ///
    /// The slot is the **contract's** choice, not this function's. What is added here is the two
    /// things a mapping has to do around it: reconcile the loans the reader has published so the
    /// contract's notion of *acquired* is the reader's rather than this process's guess, and copy
    /// the pixels in once a slot has been chosen.
    pub fn publish(
        &mut self,
        sequence: u64,
        stamp_ns: u64,
        pixels: &[u8],
    ) -> Result<FrameHandle, PoolError> {
        if pixels.len() > self.geometry.slot_bytes {
            return Err(PoolError::TooLarge {
                needed: pixels.len(),
                capacity: self.geometry.slot_bytes,
            });
        }
        self.reconcile();

        let identity = FrameIdentity {
            stream: self.stream.clone(),
            sequence,
            stamp_ns,
        };
        // A refusal here is §47.6's `pool_exhausted` — every slot is on loan — and it is *not* an
        // error in this crate's sense. §47.4: capture is never stalled. The contract has already
        // counted the drop, so the caller's correct response is to carry on with the next frame.
        let handle = match self.pool.publish(identity) {
            Ok(handle) => handle,
            Err(refusal) => return Err(PoolError::Refused(refusal.code)),
        };

        let bytes = self.frames.bytes_slice();
        let descriptor = descriptor_at(handle.slot);
        // The generation goes **first and alone**, before the pixels, and the length goes last.
        // That ordering is what `Reader::peek`'s tear check relies on: a reader that saw the old
        // generation, read bytes and then saw a new one knows the frame moved under it. A writer
        // that stamped the generation last would let a reader see a stable generation across a
        // half-written frame.
        write_u64(bytes, descriptor + D_GENERATION, handle.generation);
        let start = pixels_at(&self.geometry, handle.slot);
        for (index, byte) in pixels.iter().enumerate() {
            bytes[start + index].store(*byte, Ordering::Relaxed);
        }
        write_u64(bytes, descriptor + D_SEQUENCE, sequence);
        write_u64(bytes, descriptor + D_STAMP, stamp_ns);
        write_u64(bytes, descriptor + D_LEN, pixels.len() as u64);
        write_u64(bytes, OFF_LATEST, handle.slot as u64);

        Ok(handle)
    }

    /// Make the contract's `acquired` set equal what the reader has actually claimed.
    ///
    /// The one place the two processes' views are joined, and it is deliberately a **reconcile**
    /// rather than a message: the reader writes a claim whenever it likes and this runs whenever
    /// capture publishes, so neither waits for the other and a reader that stopped running leaves
    /// nothing to time out. A claim naming a generation the slot no longer holds is not a loan —
    /// which is what makes a reader that died mid-frame harmless, and what a boolean flag could not
    /// have expressed.
    fn reconcile(&mut self) {
        let loans = Loans::of(&self.loans, self.geometry.slots);
        let bytes = self.frames.bytes_slice();
        for slot in 0..self.geometry.slots {
            let generation = read_u64(bytes, descriptor_at(slot) + D_GENERATION);
            let claimed = loans.claim(slot) == generation && generation != 0;
            let handle = FrameHandle {
                stream: self.stream.clone(),
                slot,
                generation,
            };
            if claimed {
                // Idempotent by the contract's own rule: a second acquire of a live loan is
                // refused by name, and that refusal is exactly what "already reconciled" looks
                // like here.
                let _ = self.pool.acquire(&handle);
            } else {
                let _ = self.pool.release(&handle);
            }
        }
    }
}

/// The reading half: a policy maps it and takes loans on it.
///
/// Its frames mapping carries no `PROT_WRITE`, so *the policy cannot alter a frame* is a property
/// of the page tables rather than of an agreement. Its loans mapping does, and that is the whole of
/// what it may write.
pub struct Reader {
    frames: Mapping,
    loans: Mapping,
    geometry: Geometry,
    stream: String,
}

impl Reader {
    /// Map a pool somebody else created, from the two descriptors they passed.
    ///
    /// **Takes ownership of both**, like every `adopt` in `skynet-ring-shm`. The header is
    /// validated before this returns, so a caller never holds a `Reader` over something that is not
    /// a pool this build can address.
    #[cfg(target_os = "linux")]
    pub fn adopt(
        frames_fd: core::ffi::c_int,
        loans_fd: core::ffi::c_int,
        geometry: Geometry,
    ) -> Result<Reader, PoolError> {
        geometry.check()?;
        let frames = Mapping::adopt_bytes(
            frames_fd,
            geometry.frame_bytes(),
            geometry.slots,
            Access::Read,
        )
        .map_err(PoolError::Shm)?;
        let loans = Mapping::adopt_bytes(
            loans_fd,
            geometry.loan_bytes(),
            geometry.slots,
            Access::Write,
        )
        .map_err(PoolError::Shm)?;

        let bytes = frames.bytes_slice();
        if read_u64(bytes, OFF_MAGIC) != MAGIC || read_u32(bytes, OFF_VERSION) != VERSION {
            return Err(PoolError::NotAPool);
        }
        // The geometry the creator sized against, checked rather than trusted: a receiver that
        // disagreed would address the wrong bytes for every slot past the first.
        if read_u32(bytes, OFF_SLOTS) as usize != geometry.slots
            || read_u64(bytes, OFF_SLOT_BYTES) as usize != geometry.slot_bytes
        {
            return Err(PoolError::NotAPool);
        }

        let mut name = Vec::new();
        for index in 0..MAX_NAME {
            let byte = bytes[HEADER_BYTES + index].load(Ordering::Relaxed);
            if byte == 0 {
                break;
            }
            name.push(byte);
        }
        let stream = String::from_utf8(name).map_err(|_| PoolError::NotAPool)?;

        Ok(Reader {
            frames,
            loans,
            geometry,
            stream,
        })
    }

    /// Map a pool from its two descriptors alone, taking the geometry from the pool's own header.
    ///
    /// This is what a receiver on the far side of §25's socket uses, and it exists so that the
    /// geometry does **not** have to cross out of band. It could have: `SKYNET_RING_CHANNELS`
    /// already crosses beside the rings, and a `SKYNET_FRAME_POOL=<slots>x<bytes>` beside it would
    /// have been four lines. It would also have been a second copy of a number the region already
    /// states, and the failure when the two disagree is not an error — it is a reader addressing
    /// the wrong offset for every slot past the first and finding plausible pixels there.
    ///
    /// The header is read through a **separate, short mapping** of a duplicate descriptor, which is
    /// the whole trick: [`Mapping::adopt_bytes`] needs a byte count before it can map, and 64 bytes
    /// is a count that is correct for every pool this version can address, because [`MAGIC`] and
    /// [`VERSION`] are inside it. A pool whose header this build does not recognise is refused
    /// there, before a byte of it has been addressed as pixels.
    ///
    /// **Takes ownership of both descriptors**, on every path including the refusals — a receiver
    /// that declined a pool and kept its descriptors would hold the writer's memory alive for the
    /// life of the process while using none of it.
    #[cfg(target_os = "linux")]
    pub fn adopt_declared(
        frames_fd: core::ffi::c_int,
        loans_fd: core::ffi::c_int,
    ) -> Result<Reader, PoolError> {
        let declared = Reader::declared_geometry(frames_fd);
        match declared {
            Ok(geometry) => Reader::adopt(frames_fd, loans_fd, geometry),
            Err(error) => {
                skynet_ring_shm::pass::close(frames_fd);
                skynet_ring_shm::pass::close(loans_fd);
                Err(error)
            }
        }
    }

    /// The geometry the creator wrote into the header, read without disturbing `frames_fd`.
    #[cfg(target_os = "linux")]
    fn declared_geometry(frames_fd: core::ffi::c_int) -> Result<Geometry, PoolError> {
        // Duplicated, because `adopt_bytes` consumes what it is given and the caller still needs
        // the original to map the whole region with.
        let probe_fd = skynet_ring_shm::pass::dup(frames_fd).map_err(PoolError::Shm)?;
        // One slot of no consequence: nothing below indexes a slot, and the size is the header's.
        let probe = Mapping::adopt_bytes(probe_fd, HEADER_BYTES, 1, Access::Read)
            .map_err(PoolError::Shm)?;
        let bytes = probe.bytes_slice();
        if read_u64(bytes, OFF_MAGIC) != MAGIC || read_u32(bytes, OFF_VERSION) != VERSION {
            return Err(PoolError::NotAPool);
        }
        let geometry = Geometry {
            slots: read_u32(bytes, OFF_SLOTS) as usize,
            slot_bytes: read_u64(bytes, OFF_SLOT_BYTES) as usize,
        };
        geometry.check()?;
        Ok(geometry)
    }

    /// Which stream this pool carries.
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The newest published handle, or `None` for *no frame yet*.
    ///
    /// Never a frame of zeros: an unpublished pool carries a sentinel and answers `None` (§47.6).
    pub fn latest(&self) -> Option<FrameHandle> {
        let bytes = self.frames.bytes_slice();
        let slot = read_u64(bytes, OFF_LATEST);
        if slot == u64::MAX || slot as usize >= self.geometry.slots {
            return None;
        }
        let slot = slot as usize;
        Some(FrameHandle {
            stream: self.stream.clone(),
            slot,
            generation: read_u64(bytes, descriptor_at(slot) + D_GENERATION),
        })
    }

    /// Read a frame **without** taking a loan, refusing if it moved while being read.
    ///
    /// For a policy that wants the freshest pixels and no commitment. The tear check is
    /// `skynet-ring`'s seqlock discipline: read the generation, read the bytes, read it again. A
    /// writer that took this slot mid-read moves the generation, and the read is refused rather
    /// than returned as a smear of two frames.
    ///
    /// `None` is *this frame is gone*, which on a busy pool is an ordinary answer and not a fault.
    /// A caller that cannot tolerate it wants [`Reader::acquire`], which is what loans are for.
    pub fn peek(&self, handle: &FrameHandle, out: &mut Vec<u8>) -> Option<FrameIdentity> {
        if handle.slot >= self.geometry.slots {
            return None;
        }
        let bytes = self.frames.bytes_slice();
        let descriptor = descriptor_at(handle.slot);
        // Cleared before anything is decided, so a refusal never leaves the caller holding the
        // *previous* frame's pixels. Both refusal paths below return `None`, and a caller that
        // checked the return value would be fine either way — but one that reused the buffer and
        // looked at its length would be reading a frame it was told it could not have.
        out.clear();
        let before = read_u64(bytes, descriptor + D_GENERATION);
        if before != handle.generation {
            return None;
        }
        let identity = self.identity_at(handle.slot);
        let len = self.len_at(handle.slot);
        let start = pixels_at(&self.geometry, handle.slot);
        out.reserve(len);
        for index in 0..len {
            out.push(bytes[start + index].load(Ordering::Relaxed));
        }
        if read_u64(bytes, descriptor + D_GENERATION) != before {
            out.clear();
            return None;
        }
        Some(identity)
    }

    /// Take a loan on a slot, so the writer will not reuse it while this reader holds it.
    ///
    /// The loan is published **before** the caller reads a byte, which is the ordering that makes
    /// it worth anything: a loan taken afterwards would protect a read that had already happened.
    ///
    /// It is refused when the slot has already moved on — the generation this handle names is not
    /// the one in the slot — because that is §47.6's `stale_handle` and the alternative is showing
    /// somebody else's frame.
    pub fn acquire(&self, handle: &FrameHandle) -> Result<FrameIdentity, PoolError> {
        if handle.slot >= self.geometry.slots {
            return Err(PoolError::Refused(FrameRefusalCode::UnknownSlot));
        }
        let bytes = self.frames.bytes_slice();
        let loans = Loans::of(&self.loans, self.geometry.slots);
        loans.set(handle.slot, handle.generation);
        // Re-checked after the claim is visible, never before. A writer that took the slot between
        // the check and the claim would leave this reader holding a loan on somebody else's frame,
        // and the claim is worthless unless it is published first and validated second.
        if read_u64(bytes, descriptor_at(handle.slot) + D_GENERATION) != handle.generation {
            loans.set(handle.slot, 0);
            return Err(PoolError::Refused(FrameRefusalCode::StaleHandle));
        }
        Ok(self.identity_at(handle.slot))
    }

    /// The bytes of a slot this reader holds a loan on.
    ///
    /// No tear check, and none is needed: the loan is what makes the slot stable. A caller that has
    /// not acquired gets whatever is there, which is why this takes the handle it was issued and
    /// verifies the generation rather than trusting the caller to have called `acquire`.
    pub fn read(&self, handle: &FrameHandle, out: &mut Vec<u8>) -> Option<FrameIdentity> {
        if handle.slot >= self.geometry.slots {
            return None;
        }
        let bytes = self.frames.bytes_slice();
        if read_u64(bytes, descriptor_at(handle.slot) + D_GENERATION) != handle.generation {
            return None;
        }
        let identity = self.identity_at(handle.slot);
        let len = self.len_at(handle.slot);
        let start = pixels_at(&self.geometry, handle.slot);
        out.clear();
        out.reserve(len);
        for index in 0..len {
            out.push(bytes[start + index].load(Ordering::Relaxed));
        }
        Some(identity)
    }

    /// Give a slot back, so the writer may reuse it.
    ///
    /// §47.6: *"the pool may reuse the slot the moment it is released"*. Everything this reader
    /// still wants from those bytes must be copied out first, and the signature says so — `read`
    /// hands back an owned `Vec` rather than a borrow into the mapping, so there is no reference
    /// left to dangle when the writer takes the slot on its next publish.
    pub fn release(&self, handle: &FrameHandle) {
        Loans::of(&self.loans, self.geometry.slots).set(handle.slot, 0);
    }

    fn identity_at(&self, slot: usize) -> FrameIdentity {
        let bytes = self.frames.bytes_slice();
        let descriptor = descriptor_at(slot);
        FrameIdentity {
            stream: self.stream.clone(),
            sequence: read_u64(bytes, descriptor + D_SEQUENCE),
            stamp_ns: read_u64(bytes, descriptor + D_STAMP),
        }
    }

    /// How many bytes the slot's frame occupies.
    ///
    /// Separate from [`FrameIdentity`] on purpose, and it is worth a sentence because the
    /// alternative is tempting: a length is a fact about **this mapping**, not about the frame.
    /// §47.6's identity is what a frame *is* — the stream it arrived on, when the sensor took it
    /// and where it sits in the sequence — and it is reproduced byte-identically by three editions
    /// that hold no pixels at all. Adding a byte count to it would put a property of one operating
    /// system's transport into a document the corpus pins.
    fn len_at(&self, slot: usize) -> usize {
        let bytes = self.frames.bytes_slice();
        (read_u64(bytes, descriptor_at(slot) + D_LEN) as usize).min(self.geometry.slot_bytes)
    }
}

#[cfg(test)]
mod tests;
