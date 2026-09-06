//! The preallocated ring (§13, §28).
//!
//! > `/blackbox` is preallocated on purpose: a black box that stops recording because a log filled
//! > the disk is a black box that fails exactly when something is going wrong.
//!
//! Preallocation is the whole reason this is a ring and not a growing file, so it is worth being
//! exact about what it buys. The bytes are claimed at install, from a filesystem that had them;
//! after that, recording never asks for anything. There is no allocation on the write path, no
//! `ENOSPC` to handle in a control loop, and no interaction between "the telemetry queue filled the
//! disk" and "the incident was recorded". The ring is the same size on the worst day as on the
//! first.
//!
//! # This type does no I/O
//!
//! [`Ring`] owns no buffer and no file. It holds the geometry and the next sequence number, and
//! every method is handed the bytes it should work on. `skynetd-io` backs it with a preallocated
//! file, the suite backs it with a `Vec`, and both are exercising the same arithmetic — which is
//! the same split, and for the same reason, as `skynetd` and `skynetd-io` (§33.1).
//!
//! The write path is deliberately shaped for the file case: [`Ring::stage`] returns the byte offset
//! to write at, so a recorder does one positioned write of one record and never rewrites the file.
//!
//! # Where the header lives, and why it is not a record
//!
//! The header holds what a recording means: which robot, which body, which OS image, and the table
//! of policy digests the cycle records index into. None of it can live in a record, because records
//! are overwritten — a recording that had wrapped past its own metadata would be a stream of
//! numbers with nothing to say what they were of.
//!
//! It is padded to 4 KiB so the record area starts on a page boundary. That is not for speed. It is
//! so that a header rewrite — which happens when a new policy digest appears, and at no other time
//! — touches one page and cannot land in the middle of a record.

use alloc::vec;
use alloc::vec::Vec;

use crate::crc32::crc32;
use crate::record::{
    decode, encode, get_text, get_u16, get_u32, get_u64, put_text, put_u16, put_u32, put_u64,
    Entry, Record, RecordError, Shape,
};
use crate::FORMAT_VERSION;

/// `SKYNETBB`, the first eight bytes of any ring.
pub const MAGIC: &[u8; 8] = b"SKYNETBB";

/// Scalar fields of the header, before the two tables.
pub const HEADER_FIXED: usize = 184;

// The header's field offsets, named rather than written out at each use.
//
// The first draft of this file spelled them as literals at the read and again at the write, and put
// `header_len` and `record_size` on top of each other — a bug that produces a ring which opens,
// passes its CRC, and reports a record size read four bytes into the wrong field. Two lists of
// numbers that have to agree is one list too many.
mod at {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 8;
    pub const HEADER_LEN: usize = 12;
    pub const RECORD_SIZE: usize = 16;
    pub const CAPACITY: usize = 20;
    pub const JOINTS: usize = 24;
    pub const CHANNELS: usize = 26;
    pub const TIER: usize = 28;
    pub const ENFORCEMENT: usize = 29;
    pub const SIMULATOR: usize = 30;
    pub const EMBODIMENT: usize = 32;
    pub const OS_IMAGE: usize = 64;
    pub const ROBOT_ID: usize = 96;
    pub const DIGEST_CAPACITY: usize = 160;
    pub const CAPABILITY_CAPACITY: usize = 162;
    pub const WALL_EPOCH_NS: usize = 164;
    pub const MONO_NS: usize = 172;
    pub const CRC: usize = 180;
}

/// The header is padded to this, so the record area begins on a page boundary.
pub const HEADER_ALIGN: usize = 4096;

/// The fixed width of a robot identifier in the header.
pub const ROBOT_ID: usize = 64;

/// The fixed width of a capability token in the header table.
///
/// The same width as a channel name and for the same reason: a token is a dotted name from the same
/// vocabulary, and a table indexed by position needs a fixed stride.
pub const TOKEN_NAME: usize = 48;

/// The fixed width of an action channel's name in the header table.
///
/// Forty-eight bytes, which holds every name the role vocabulary can produce — the longest
/// canonical form, `left_arm.wrist.rotation`, is 23 — with room for the `aux.` names a vendor
/// invents. A name that does not fit is truncated and the truncation is visible, which is a better
/// failure than a variable-length table nothing can index into by position.
pub const CHANNEL_NAME: usize = 48;

/// What the ring cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingError {
    /// The byte budget does not hold the header and at least one record.
    ///
    /// Refused rather than rounded up to something that fits. A ring of two records is not a black
    /// box, and a robot whose disk cannot spare the space should be told at install rather than
    /// discovering it during an incident.
    TooSmall {
        /// What was offered.
        budget: usize,
        /// What the header and one record need.
        needed: usize,
    },
    /// The bytes handed over do not begin with [`MAGIC`].
    NotARing,
    /// The header was written by a format version this edition does not read.
    Version(u16),
    /// The header's own CRC does not match: the metadata is damaged, so nothing below it can be
    /// trusted to mean what it says.
    HeaderTorn,
    /// The store is shorter than the header says the ring is.
    Short {
        /// What the header describes.
        expected: usize,
        /// What was handed over.
        found: usize,
    },
    /// A record could not be encoded or decoded.
    Record(RecordError),
    /// The policy digest table is full.
    ///
    /// Reported rather than silently reusing a slot: a cycle whose policy index pointed at the
    /// wrong digest would attribute one policy's behaviour to another, which is the single most
    /// misleading thing this file could contain.
    DigestTableFull,
    /// The capability token table is full, for the same reason and with the same consequence.
    TokenTableFull,
    /// A claimed sequence number was zero (§26.2).
    ///
    /// Sequences start at 1, and [`Ring::offset_of`] subtracts one before taking the remainder, so
    /// a zero is not a slot — it is a claim counter that was read before whoever created it had
    /// seeded it. `skynet_claim`'s own header names that race and says a zero is the way it shows:
    /// a writer that attached between the creator's `open` and its `store` gets one. Refused here
    /// rather than wrapped around to the last slot in the ring, which is where the arithmetic would
    /// otherwise put it — on top of the oldest record, silently.
    Unseeded,
}

/// The arithmetic of a ring, before any bytes exist.
///
/// Separated from [`Ring`] so a provisioning step can ask what a budget buys without creating
/// anything — which is what §28's "preallocated at install" needs, and what makes the answer to
/// "is an hour of this robot going to fit" a computation rather than an assumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// The body this ring records.
    pub shape: Shape,
    /// Bytes per record.
    pub record_size: usize,
    /// How many records the ring holds before it wraps.
    pub capacity: usize,
    /// Bytes before the first record.
    pub header_len: usize,
    /// How many policy digests the header table holds.
    pub digests: u16,
    /// How many capability tokens the header table holds.
    pub capabilities: u16,
}

impl Geometry {
    /// Fit a ring into `budget` bytes.
    ///
    /// The capacity is whatever is left after the header, rounded *down*. A ring that overran its
    /// budget by one record would be a preallocation that did not preallocate.
    pub fn for_budget(
        shape: Shape,
        digests: u16,
        capabilities: u16,
        budget: usize,
    ) -> Result<Geometry, RingError> {
        let record_size = shape.record_size();
        let header_len = header_len(shape, digests, capabilities);
        let needed = header_len + record_size;
        if budget < needed {
            return Err(RingError::TooSmall { budget, needed });
        }
        Ok(Geometry {
            shape,
            record_size,
            capacity: (budget - header_len) / record_size,
            header_len,
            digests,
            capabilities,
        })
    }

    /// Total bytes: what to preallocate.
    pub fn total_bytes(&self) -> usize {
        self.header_len + self.capacity * self.record_size
    }

    /// How long a window this is, in seconds, at `rate_hz` records a second.
    ///
    /// The function §13's "sized for roughly an hour" turns into when somebody needs the number. A
    /// 30-joint humanoid at 1 kHz writes about 800 bytes a millisecond, so an hour is near 3 GB and
    /// a 512 MB ring holds about ten minutes. Both facts are worth knowing before the incident, and
    /// neither is knowable from a constant named `ONE_HOUR`.
    pub fn seconds_covered(&self, rate_hz: f64) -> f64 {
        if rate_hz <= 0.0 {
            return 0.0;
        }
        self.capacity as f64 / rate_hz
    }

    /// The budget that would buy `seconds` at `rate_hz`.
    ///
    /// The inverse, for a provisioning step that has a duration in hand and needs a size.
    pub fn budget_for(
        shape: Shape,
        digests: u16,
        capabilities: u16,
        rate_hz: f64,
        seconds: f64,
    ) -> usize {
        // Rounded up without `f64::ceil`, which lives in `std` and this crate is `no_std`. A
        // budget rounded *down* would buy less than the duration asked for, and being told a ring
        // holds an hour when it holds fifty-nine minutes is the class of quiet shortfall the whole
        // sizing API exists to prevent.
        let exact = rate_hz * seconds;
        let truncated = if exact > 0.0 { exact as usize } else { 0 };
        let records = if (truncated as f64) < exact {
            truncated + 1
        } else {
            truncated.max(1)
        };
        header_len(shape, digests, capabilities) + records * shape.record_size()
    }
}

fn header_len(shape: Shape, digests: u16, capabilities: u16) -> usize {
    let raw = HEADER_FIXED
        + digests as usize * 32
        + capabilities as usize * TOKEN_NAME
        + shape.channels as usize * CHANNEL_NAME;
    raw.div_ceil(HEADER_ALIGN) * HEADER_ALIGN
}

/// What a recording is *of*.
///
/// Every field here is something an investigator needs and cannot recover from the records: which
/// robot, which body, which image, and what the kernel was allowed to do at the time. A sealed file
/// missing any of them is a stream of numbers.
#[derive(Debug, Clone)]
pub struct Meta {
    /// The device identity this robot was provisioned with (§50).
    pub robot_id: alloc::string::String,
    /// The embodiment descriptor's digest — half of what makes replay meaningful (§56.4).
    pub embodiment: [u8; 32],
    /// The running OS image's digest.
    pub os_image: [u8; 32],
    /// The tier in force, as `skynet_kernel::Tier::as_str` spells it.
    pub tier: u8,
    /// The enforcement mode in force.
    pub enforcement: u8,
    /// Whether the RHAL driver declared itself a simulator (§35.3, §70).
    ///
    /// In the header rather than inferred, because a benchmark result replayed from a simulated
    /// recording must never be mistaken for a field one (§56.2).
    pub simulator: bool,
    /// `CLOCK_REALTIME` when the ring was opened, in nanoseconds since the epoch.
    ///
    /// Recorded once, and used only to place the recording on a wall clock at seal time. Nothing in
    /// the control path reads it (§35.1).
    pub wall_epoch_ns: u64,
    /// `CLOCK_MONOTONIC_RAW` at the same instant, which is what makes the correlation usable.
    pub mono_ns: u64,
    /// The action channel names, in the order the records' arrays are indexed by.
    pub channels: Vec<alloc::string::String>,
}

/// A live ring: geometry, metadata, and where the next record goes.
#[derive(Debug, Clone)]
pub struct Ring {
    geometry: Geometry,
    meta: Meta,
    digests: Vec<[u8; 32]>,
    capabilities: Vec<alloc::string::String>,
    next: u64,
}

impl Ring {
    /// Create a ring and produce the bytes to preallocate.
    ///
    /// The returned buffer is the whole file: a header, then `capacity * record_size` zero bytes.
    /// Zero is not a valid record — every record's CRC covers a non-zero kind byte — so a fresh
    /// ring reads as empty rather than as a very long recording of nothing.
    pub fn create(geometry: Geometry, meta: Meta) -> (Ring, Vec<u8>) {
        let ring = Ring {
            geometry,
            meta,
            digests: Vec::new(),
            capabilities: Vec::new(),
            next: 1,
        };
        let mut store = vec![0u8; geometry.total_bytes()];
        ring.write_header(&mut store);
        (ring, store)
    }

    /// Read a ring back out of a store, and find where the recording got to.
    ///
    /// Recovery is a scan, and it is a scan on purpose. The alternative is a "next sequence" field
    /// in the header, updated on every append — which turns one positioned write per record into
    /// two, and makes the header the thing most likely to be torn by a power cut. The scan is
    /// `O(capacity)`, happens once at start, and is the only way to be right after a crash: the
    /// highest sequence number actually on disk is the truth, whatever any counter claims.
    pub fn open(store: &[u8]) -> Result<Ring, RingError> {
        if store.len() < HEADER_FIXED || &store[at::MAGIC..at::MAGIC + 8] != MAGIC {
            return Err(RingError::NotARing);
        }
        let version = get_u16(store, at::VERSION);
        if version != FORMAT_VERSION {
            return Err(RingError::Version(version));
        }
        let header_len = get_u32(store, at::HEADER_LEN) as usize;
        if header_len < HEADER_FIXED || store.len() < header_len {
            return Err(RingError::Short {
                expected: header_len.max(HEADER_FIXED),
                found: store.len(),
            });
        }
        let mut header = store[..header_len].to_vec();
        let stated = get_u32(&header, at::CRC);
        header[at::CRC..at::CRC + 4].fill(0);
        if crc32(&header) != stated {
            return Err(RingError::HeaderTorn);
        }

        let shape = Shape {
            joints: get_u16(store, at::JOINTS),
            channels: get_u16(store, at::CHANNELS),
        };
        let digests_capacity = get_u16(store, at::DIGEST_CAPACITY);
        let capabilities_capacity = get_u16(store, at::CAPABILITY_CAPACITY);
        let geometry = Geometry {
            shape,
            record_size: get_u32(store, at::RECORD_SIZE) as usize,
            capacity: get_u32(store, at::CAPACITY) as usize,
            header_len,
            digests: digests_capacity,
            capabilities: capabilities_capacity,
        };
        let expected = geometry.total_bytes();
        if store.len() < expected {
            return Err(RingError::Short {
                expected,
                found: store.len(),
            });
        }

        let mut embodiment = [0u8; 32];
        embodiment.copy_from_slice(&store[at::EMBODIMENT..at::EMBODIMENT + 32]);
        let mut os_image = [0u8; 32];
        os_image.copy_from_slice(&store[at::OS_IMAGE..at::OS_IMAGE + 32]);
        let robot_id = get_text(store, at::ROBOT_ID, ROBOT_ID);

        let digest_base = HEADER_FIXED;
        let token_base = digest_base + digests_capacity as usize * 32;
        let channel_base = token_base + capabilities_capacity as usize * TOKEN_NAME;
        let mut digests = Vec::new();
        for index in 0..digests_capacity as usize {
            let at = digest_base + index * 32;
            let mut digest = [0u8; 32];
            digest.copy_from_slice(&store[at..at + 32]);
            // The table is filled in order and never has a hole, so the first all-zero entry is the
            // end of it. A real digest of thirty-two zero bytes is a preimage nobody has.
            if digest == [0u8; 32] {
                break;
            }
            digests.push(digest);
        }
        let mut capabilities = Vec::new();
        for index in 0..capabilities_capacity as usize {
            let token = get_text(store, token_base + index * TOKEN_NAME, TOKEN_NAME);
            // Filled in order, so the first empty entry ends the table. A capability token cannot be
            // the empty string: `parse_role` refuses one.
            if token.is_empty() {
                break;
            }
            capabilities.push(token);
        }
        let mut channels = Vec::new();
        for index in 0..shape.channels as usize {
            channels.push(get_text(
                store,
                channel_base + index * CHANNEL_NAME,
                CHANNEL_NAME,
            ));
        }

        let meta = Meta {
            robot_id,
            embodiment,
            os_image,
            tier: store[at::TIER],
            enforcement: store[at::ENFORCEMENT],
            simulator: store[at::SIMULATOR] != 0,
            wall_epoch_ns: get_u64(store, at::WALL_EPOCH_NS),
            mono_ns: get_u64(store, at::MONO_NS),
            channels,
        };

        let mut ring = Ring {
            geometry,
            meta,
            digests,
            capabilities,
            next: 1,
        };
        let highest = ring
            .records(store)
            .filter_map(|(_, record)| record.ok())
            .map(|record| record.sequence)
            .max()
            .unwrap_or(0);
        ring.next = highest + 1;
        Ok(ring)
    }

    /// The geometry this ring was created with.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// What the recording is of.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// The policy digests the cycle records' indices point at.
    pub fn digests(&self) -> &[[u8; 32]] {
        &self.digests
    }

    /// The capability tokens the cycle records' indices point at.
    pub fn capabilities(&self) -> &[alloc::string::String] {
        &self.capabilities
    }

    /// Find or add a capability token, returning the index a cycle should record.
    ///
    /// The same shape as [`Ring::intern_digest`], and rare for the same reason: a token is fixed
    /// for the life of a lease, not chosen per cycle.
    pub fn intern_capability(&mut self, store: &mut [u8], token: &str) -> Result<u16, RingError> {
        if let Some(index) = self.capabilities.iter().position(|held| held == token) {
            return Ok(index as u16);
        }
        if self.capabilities.len() >= self.geometry.capabilities as usize {
            return Err(RingError::TokenTableFull);
        }
        self.capabilities.push(token.into());
        self.write_header(store);
        Ok((self.capabilities.len() - 1) as u16)
    }

    /// The sequence number the next appended record will carry.
    pub fn next_sequence(&self) -> u64 {
        self.next
    }

    /// Find or add `digest` in the header table, returning the index a cycle should record.
    ///
    /// Called when a policy starts, not per cycle. The header is rewritten when the table grows,
    /// which is why the caller is handed the whole store: a new digest is a metadata change and it
    /// must be durable before any record referring to it is.
    pub fn intern_digest(&mut self, store: &mut [u8], digest: [u8; 32]) -> Result<u16, RingError> {
        if let Some(index) = self.digests.iter().position(|held| *held == digest) {
            return Ok(index as u16);
        }
        if self.digests.len() >= self.geometry.digests as usize {
            return Err(RingError::DigestTableFull);
        }
        self.digests.push(digest);
        self.write_header(store);
        Ok((self.digests.len() - 1) as u16)
    }

    /// The byte offset of the slot a given sequence number lives in.
    pub fn offset_of(&self, sequence: u64) -> usize {
        let slot = ((sequence - 1) % self.geometry.capacity as u64) as usize;
        self.geometry.header_len + slot * self.geometry.record_size
    }

    /// Encode the next record into `slot`, and say where it goes.
    ///
    /// The shape the file-backed recorder wants: it owns one scratch buffer of `record_size` bytes,
    /// calls this, and does one positioned write. Nothing here allocates and nothing here reads a
    /// clock — `mono_ns` is supplied, because §35.2 says the kernel owns no clock and the recorder
    /// beside it should not have opinions the kernel is not allowed to have.
    pub fn stage(
        &mut self,
        slot: &mut [u8],
        mono_ns: u64,
        entry: Entry<'_>,
    ) -> Result<(u64, usize), RingError> {
        let sequence = self.next;
        encode(self.geometry.shape, slot, sequence, mono_ns, entry).map_err(RingError::Record)?;
        let offset = self.offset_of(sequence);
        self.next += 1;
        Ok((sequence, offset))
    }

    /// Encode a record into `slot` at a sequence **claimed somewhere else**, and say where it goes.
    ///
    /// [`Ring::stage`] takes the sequence from this `Ring`'s own counter, which is in memory and is
    /// therefore correct for exactly one writer. §26.2 has two — `skynetd` records events and gaps,
    /// `skynet-kernel-host` records cycles, and both belong in one recording — so on a robot with a
    /// control loop the number comes from the `AtomicU64` both processes map (`skynet-claim`) and
    /// arrives here already claimed.
    ///
    /// # What this does to the ring's own counter
    ///
    /// It raises it to `sequence + 1` and never lowers it, so `next_sequence()` stays a floor
    /// rather than becoming a lie: a writer that has staged sequence 900 knows the recording has at
    /// least 900 records in it, and does **not** know what the other writer has claimed since. A
    /// caller that needs the true next sequence asks the claim counter, which is the only thing
    /// that has it. Recovery is unaffected either way — `Ring::open` scans, on purpose, and the
    /// highest sequence on disk is the truth whatever any counter says.
    pub fn stage_claimed(
        &mut self,
        slot: &mut [u8],
        mono_ns: u64,
        entry: Entry<'_>,
        sequence: u64,
    ) -> Result<usize, RingError> {
        if sequence == 0 {
            return Err(RingError::Unseeded);
        }
        encode(self.geometry.shape, slot, sequence, mono_ns, entry).map_err(RingError::Record)?;
        let offset = self.offset_of(sequence);
        if sequence >= self.next {
            self.next = sequence + 1;
        }
        Ok(offset)
    }

    /// Append into a store held in memory.
    ///
    /// The convenience [`Ring::stage`] does not offer, for callers whose ring is a buffer rather
    /// than a file.
    pub fn append(
        &mut self,
        store: &mut [u8],
        mono_ns: u64,
        entry: Entry<'_>,
    ) -> Result<u64, RingError> {
        let sequence = self.next;
        let offset = self.offset_of(sequence);
        let size = self.geometry.record_size;
        if store.len() < offset + size {
            return Err(RingError::Short {
                expected: offset + size,
                found: store.len(),
            });
        }
        encode(
            self.geometry.shape,
            &mut store[offset..offset + size],
            sequence,
            mono_ns,
            entry,
        )
        .map_err(RingError::Record)?;
        self.next += 1;
        Ok(sequence)
    }

    /// Read the slot at `index`, whatever it holds.
    pub fn read_slot(&self, store: &[u8], index: usize) -> Result<Record, RecordError> {
        let at = self.geometry.header_len + index * self.geometry.record_size;
        let end = at + self.geometry.record_size;
        if store.len() < end {
            return Err(RecordError::Truncated);
        }
        decode(self.geometry.shape, &store[at..end])
    }

    /// Every slot, in slot order, with whatever each one decoded to.
    ///
    /// Slot order rather than sequence order, and the failures are handed back rather than skipped:
    /// a caller counting damaged slots is asking a question about the recording's completeness
    /// (§41.1), and a reader that silently dropped them would answer it wrongly.
    pub fn records<'a>(
        &'a self,
        store: &'a [u8],
    ) -> impl Iterator<Item = (usize, Result<Record, RecordError>)> + 'a {
        (0..self.geometry.capacity).map(move |index| (index, self.read_slot(store, index)))
    }

    /// Every record the ring holds, oldest first.
    ///
    /// Sorted by sequence number rather than by slot, because the wrap point is wherever the
    /// recording happened to be — and an investigator reading a file that starts in the middle of
    /// Tuesday is an investigator who will misread it.
    pub fn ordered(&self, store: &[u8]) -> Vec<Record> {
        let mut held: Vec<Record> = self
            .records(store)
            .filter_map(|(_, record)| record.ok())
            .collect();
        held.sort_by_key(|record| record.sequence);
        held
    }

    fn write_header(&self, store: &mut [u8]) {
        let header_len = self.geometry.header_len;
        let header = &mut store[..header_len];
        header.fill(0);
        header[at::MAGIC..at::MAGIC + 8].copy_from_slice(MAGIC);
        put_u16(header, at::VERSION, FORMAT_VERSION);
        put_u32(header, at::HEADER_LEN, header_len as u32);
        put_u32(header, at::RECORD_SIZE, self.geometry.record_size as u32);
        put_u32(header, at::CAPACITY, self.geometry.capacity as u32);
        put_u16(header, at::JOINTS, self.geometry.shape.joints);
        put_u16(header, at::CHANNELS, self.geometry.shape.channels);
        header[at::TIER] = self.meta.tier;
        header[at::ENFORCEMENT] = self.meta.enforcement;
        header[at::SIMULATOR] = u8::from(self.meta.simulator);
        header[at::EMBODIMENT..at::EMBODIMENT + 32].copy_from_slice(&self.meta.embodiment);
        header[at::OS_IMAGE..at::OS_IMAGE + 32].copy_from_slice(&self.meta.os_image);
        put_text(header, at::ROBOT_ID, ROBOT_ID, &self.meta.robot_id);
        put_u16(header, at::DIGEST_CAPACITY, self.geometry.digests);
        put_u16(header, at::CAPABILITY_CAPACITY, self.geometry.capabilities);
        put_u64(header, at::WALL_EPOCH_NS, self.meta.wall_epoch_ns);
        put_u64(header, at::MONO_NS, self.meta.mono_ns);

        let digest_base = HEADER_FIXED;
        for (index, digest) in self.digests.iter().enumerate() {
            let at = digest_base + index * 32;
            header[at..at + 32].copy_from_slice(digest);
        }
        let token_base = digest_base + self.geometry.digests as usize * 32;
        for (index, token) in self.capabilities.iter().enumerate() {
            put_text(header, token_base + index * TOKEN_NAME, TOKEN_NAME, token);
        }
        let channel_base = token_base + self.geometry.capabilities as usize * TOKEN_NAME;
        for (index, name) in self.meta.channels.iter().enumerate() {
            if index >= self.geometry.shape.channels as usize {
                break;
            }
            put_text(
                header,
                channel_base + index * CHANNEL_NAME,
                CHANNEL_NAME,
                name,
            );
        }

        let crc = crc32(header);
        put_u32(header, at::CRC, crc);
    }
}
