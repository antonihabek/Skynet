//! The black box — §13 and §26.
//!
//! > Every control cycle, appended to an on-disk ring buffer sized for roughly an hour […] Sealed
//! > and uploaded on any fault, estop, or operator request.
//!
//! Three things follow from that sentence, and this crate is the three of them:
//!
//! * a **ring** ([`ring`]) that a control loop can append to at joint rate without allocating,
//!   without growing, and without a filesystem underneath it;
//! * a **seal** ([`seal`]) that turns the live window into an MCAP file every robotics tool
//!   already reads;
//! * a **replay** ([`replay`]) that re-runs the recorded inputs through this edition's Safety
//!   Kernel and reports, cycle by cycle, whether it reaches the recorded verdict.
//!
//! # Why the ring is not itself an MCAP file
//!
//! It was the first thing tried and it does not work. MCAP is a linear container: a magic, a
//! header, records, a data-end, a summary computed over everything before it, a footer. A ring
//! overwrites its oldest bytes, so the file's own framing would be the first casualty of the second
//! hour — and a black box whose format breaks precisely when it wraps is a black box that works
//! only during the incidents short enough not to need it.
//!
//! So the two roles are separated. **The ring is the recording**: fixed-size records, one slot per
//! record, a sequence number that only ever increases, and no structure that a wrap can invalidate.
//! **MCAP is the artifact**: produced once, at seal, from whatever the ring holds at that instant.
//! §26 chose MCAP so that an incident opens in Foxglove with no exporter, and that is a property of
//! the file handed to an investigator, not of the bytes being written a thousand times a second.
//!
//! # What the ring costs, and the sentence in §13 it makes precise
//!
//! §13 says "sized for roughly an hour". An hour is not a size, and the conversion is the part
//! worth writing down: a record is [`record::record_size`] bytes, which for a 30-joint humanoid
//! with 30 action channels is about 800, so an hour at a 1 kHz joint rate is roughly **2.9 GB**.
//! That is not a rounding error against §28's storage budget; it is most of a small disk.
//!
//! This crate therefore sizes the ring **by a byte budget** and *computes* what that buys:
//! [`ring::Ring::seconds_covered`] answers "how long a window is this, at this rate", and
//! [`ring::Geometry`] answers it before a file is created. An operator who asked for an hour and
//! got eleven minutes should be told eleven minutes. The alternative — a constant named
//! `ONE_HOUR` and a ring that silently holds a fraction of it — is the class of number this whole
//! specification exists to stop being invented.
//!
//! # The hot path allocates nothing
//!
//! [`ring::Ring::append`] writes into a caller-provided `&mut [u8]`: no `Vec`, no `String`, no
//! formatting, no clock read. It is a memcpy and some integer arithmetic. Sealing allocates freely,
//! because sealing happens after the incident and no control deadline is waiting on it.
//!
//! The crate is `no_std` for the same reason: not because the recorder runs on the safety MCU — it
//! does not, `skynetd` owns it (§33.2) — but because a crate that cannot reach `std` cannot quietly
//! acquire a file handle, a clock or a thread in its hot path.
//!
//! # Completeness, and the gap that says it is a gap
//!
//! `telemetry.blackbox.complete` (§15, §41.1) is the test this design answers to, and the ring
//! answers it with sequence numbers rather than a promise. Every record carries one, they increase
//! by exactly one, and [`seal::Sealed::continuity`] reports what a reader is looking at: how many
//! records the window holds, the range of sequence numbers it covers, and any **gap** — a run of
//! sequence numbers the ring does not have.
//!
//! Two kinds of gap exist and they are not the same fact:
//!
//! * **Overwritten.** The window moved on. Normal, expected, and not a defect: the ring is a
//!   window and older records leave it.
//! * **Dropped.** Something could not keep up — §47.4's encoder falling behind is the canonical
//!   case — and the producer recorded [`record::Kind::Gap`] to say so. A gap annotated as a gap is
//!   honest; a gap that looks like a quiet period is not, and the difference matters when the
//!   question is why the robot appeared to do nothing for 200 ms.
//!
//! A torn record — a write interrupted by power loss — is the third case, and it is detected
//! rather than inferred: every record carries a CRC-32 over its own bytes, so a half-written slot
//! reports itself as damaged instead of deserialising into plausible nonsense.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod crc32;
pub mod mcap;
pub mod record;
pub mod replay;
pub mod ring;
pub mod schema;
pub mod seal;

#[cfg(test)]
mod tests;

pub use record::{
    Body, Cycle, CycleView, Entry, Event, EventCode, EventView, Frame, FrameView, Gap, GapReason,
    Kind, Record, Shape, Stream,
};
pub use ring::{Geometry, Meta, Ring, RingError};
pub use seal::{seal, Continuity, Hole, Reason, Sealed};

/// The ring's on-disk format version.
///
/// Bumped when the record layout changes in a way an older reader would misread. It is written into
/// the header and checked on open, because the failure it prevents — a new daemon reading an old
/// ring as though the fields were where it expects them — produces an incident report that is
/// confidently wrong, which is worse than no incident report.
///
/// **2** adds `streams_absent` to the frame record and gives `worst_skew_us` a value meaning *not
/// measurable* ([`record::SKEW_UNMEASURABLE`]), which moves the stream table four bytes. Both exist
/// because the version-1 record could not distinguish a frame whose streams were all present from
/// one whose capture paths had died — it recorded the survivors and said nothing about the rest, so
/// a recording of a robot with two dead cameras read as a recording of a robot with one camera. A
/// black box that cannot say what was missing is answering the wrong half of the question an
/// incident asks.
pub const FORMAT_VERSION: u16 = 2;

/// The library string written into every sealed file's MCAP header.
///
/// MCAP asks writers to identify themselves so that a file with a format quirk can be traced to the
/// writer that produced it. This one names the edition as well as the project, because §31.3 has
/// several and "which edition sealed this" is the first question of any divergence.
pub const LIBRARY: &str = "skynet-blackbox rust-edition 0.1.0";
