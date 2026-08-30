//! The record — one fixed-size slot in the ring.
//!
//! §13 names the fields:
//!
//! ```text
//! tick · monotonic ns · observation hash · policy digest · policy output
//!      · kernel verdict (guards tripped, clamps applied) · final command · RHAL result
//! ```
//!
//! and this module is that line, laid out to a byte offset. Four decisions carry the design.
//!
//! # Fixed size
//!
//! Every record occupies exactly [`Shape::record_size`] bytes, computed once from the body's joint
//! and channel counts. That is what makes the ring a ring: slot `n` is at
//! `header + n * record_size`, wrapping is `sequence % capacity`, and there is no index, no
//! compaction pass and no allocation. A variable-length record would need a length prefix, and a
//! length prefix that is itself overwritten by a wrap is a reader following a pointer into the
//! middle of some other record.
//!
//! It costs padding: an [`Event`] is smaller than a [`Cycle`] and is written into the same slot. On
//! any real body cycles outnumber everything else by four orders of magnitude, so the waste is a
//! rounding error against a benefit — a torn write damages exactly one record, and the reader knows
//! which — that is not.
//!
//! # A CRC per record
//!
//! Power fails mid-write. Without a check the next reader deserialises whatever the drive happened
//! to persist, and a half-written cycle is not obviously wrong: the numbers are plausible, the
//! timestamps are nearly right, and the incident report is confidently false. With one, the slot
//! reports itself as damaged and the seal says so.
//!
//! # The policy digest is an index
//!
//! §13's line says "policy digest", which is 32 bytes that are identical for hours at a time.
//! Written per cycle at 1 kHz it is 115 MB an hour of one repeated constant. It is a `u16` index
//! into a table in the ring header, so the digest is recorded once — and it is in the *header*
//! rather than in a preceding event record, because an event record is overwritten on the next wrap
//! and the recording would then know it had a policy without knowing which.
//!
//! # `desired` is `f32` and `command` is `f64`
//!
//! Not an oversight, and the asymmetry is the whole of what makes replay checkable.
//!
//! `desired` is what the policy produced, and it reached the kernel through the RHAL, whose values
//! are `f32` (§24). Recording it as `f32` records exactly what happened; widening to `f64` is
//! lossless, so a replay can hand the kernel the identical input.
//!
//! `command` is what the kernel decided, and the kernel computes in `f64`. A clamp emits
//! `signum(v) * ceiling`, which in general has no `f32` that represents it. Recording *that* as
//! `f32` would round the verdict, and a replay comparing a recomputed `f64` against a rounded `f32`
//! would report a divergence on every clamped cycle — an alarm that fires on correct behaviour,
//! which is the fastest way to make a conformance test ignored. The narrowing to `f32` happens
//! where it really happens, at the RHAL boundary, and is a deterministic function of the `f64` this
//! record holds.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use skynet_kernel::{FaultResponse, Guard, GUARDS};

/// What a slot holds.
///
/// The discriminants are on the wire and are never renumbered; a reader of an older ring depends on
/// them meaning what they meant when it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// One control cycle: §13's line, in full.
    Cycle = 1,
    /// One assembled observation frame: hashes, per-stream ages, worst skew (§25.1).
    Frame = 2,
    /// Something changed that was not a cycle: a lease, an estop, an OTA, a grant (§26).
    Event = 3,
    /// Records that should exist and do not, annotated as absent (§47.4).
    Gap = 4,
}

impl Kind {
    /// Decode a wire discriminant.
    pub fn from_u8(value: u8) -> Option<Kind> {
        match value {
            1 => Some(Kind::Cycle),
            2 => Some(Kind::Frame),
            3 => Some(Kind::Event),
            4 => Some(Kind::Gap),
            _ => None,
        }
    }

    /// The MCAP topic this kind's messages are published on, for the kinds that map to exactly one.
    ///
    /// [`Kind::Cycle`] has none: §26 fans one cycle across four channels, because an investigator
    /// asking "what did the kernel do" and one asking "what did the policy want" are looking for
    /// different streams.
    pub fn topic(self) -> Option<&'static str> {
        match self {
            Kind::Cycle => None,
            Kind::Frame => Some("/sk/frame"),
            Kind::Event | Kind::Gap => Some("/sk/event"),
        }
    }
}

/// How big a body is, and therefore how big a record is.
///
/// `joints` and `channels` are separately counted because they are separately real: the RHAL is
/// positional over joints (§24) and the kernel is named over action channels (§25), and a body
/// whose transport plan drops a channel or drives two joints from one has different counts. A
/// single `dof` would have been one number quietly standing for two things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// How many joints the RHAL reports.
    pub joints: u16,
    /// How many action channels the kernel evaluates.
    pub channels: u16,
}

/// Bytes before any kind-specific field: the CRC, the kind, the sequence, the timestamp.
pub const PREFIX: usize = 24;

/// Where a [`Cycle`]'s arrays begin — everything before it is scalar and fixed.
pub const CYCLE_FIXED: usize = 176;

/// The most guards that can trip in one cycle. Mirrors `skynet_kernel::MAX_TRIPPED`.
pub const MAX_TRIPPED: usize = 8;

/// The capability index meaning "the runtime did not supply one".
///
/// Distinct from index 0, which is a real token. §7.2 is explicit that a guard with no input
/// reports unavailable and never passes by default, so "nobody said" has to be representable —
/// collapsing it onto a token would turn an unevaluated mode gate into a passed one.
pub const NO_CAPABILITY: u16 = u16::MAX;

/// The most sensor streams one [`Frame`] record can carry ages for.
///
/// Six, which covers three cameras, an IMU, joint state and a force/torque sensor — the shape of
/// every body in §66's driver set. A body with more streams records six and the count of what it
/// left out, because a record that grew to fit would make every record in the ring grow with it.
pub const MAX_STREAMS: usize = 6;

/// The fixed width of a stream name inside a [`Frame`] record.
pub const STREAM_NAME: usize = 16;

/// The fixed width of an [`Event`]'s subject — a package id, a capability token, a slot name.
pub const EVENT_SUBJECT: usize = 64;

/// The fixed width of an [`Event`]'s detail text.
pub const EVENT_DETAIL: usize = 96;

const FRAME_BODY: usize = 48 + MAX_STREAMS * (STREAM_NAME + 8);
const EVENT_BODY: usize = 4 + EVENT_SUBJECT + EVENT_DETAIL;
const GAP_BODY: usize = 24;

impl Shape {
    /// Bytes one record occupies.
    ///
    /// The maximum over every kind, rounded up to eight. Rounding is not alignment — nothing here
    /// is read through a reference — it is so that a slot boundary lands predictably when somebody
    /// is looking at a hex dump of a damaged ring at three in the morning.
    pub fn record_size(self) -> usize {
        let channels = self.channels as usize;
        let joints = self.joints as usize;
        // desired f32 + command f64 + mask u8, the clamped bitmap, and three f32 arrays of state.
        let cycle = CYCLE_FIXED + channels * 13 + channels.div_ceil(8) + joints * 12;
        let largest = cycle
            .max(PREFIX + FRAME_BODY)
            .max(PREFIX + EVENT_BODY)
            .max(PREFIX + GAP_BODY);
        largest.div_ceil(8) * 8
    }

    /// Where each of a cycle's arrays starts, given this shape.
    fn cycle_offsets(self) -> CycleOffsets {
        let channels = self.channels as usize;
        let joints = self.joints as usize;
        let desired = CYCLE_FIXED;
        let command = desired + channels * 4;
        let mask = command + channels * 8;
        let clamped = mask + channels;
        let position = clamped + channels.div_ceil(8);
        let velocity = position + joints * 4;
        let effort = velocity + joints * 4;
        CycleOffsets {
            desired,
            command,
            mask,
            clamped,
            position,
            velocity,
            effort,
        }
    }
}

struct CycleOffsets {
    desired: usize,
    command: usize,
    mask: usize,
    clamped: usize,
    position: usize,
    velocity: usize,
    effort: usize,
}

// ── Reading and writing scalars ──────────────────────────────────────────────
//
// Little-endian, everywhere, without a runtime check. Every target in §28's table is little-endian
// and a big-endian robot would have larger problems than this crate; what matters is that the
// choice is *written down*, because a ring is read by a program that did not write it.

pub(crate) fn put_u16(buf: &mut [u8], at: usize, value: u16) {
    buf[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
pub(crate) fn put_u64(buf: &mut [u8], at: usize, value: u64) {
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn put_f32(buf: &mut [u8], at: usize, value: f32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_f64(buf: &mut [u8], at: usize, value: f64) {
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn get_u16(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}
pub(crate) fn get_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}
pub(crate) fn get_u64(buf: &[u8], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[at..at + 8]);
    u64::from_le_bytes(bytes)
}
fn get_f32(buf: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}
fn get_f64(buf: &[u8], at: usize) -> f64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[at..at + 8]);
    f64::from_le_bytes(bytes)
}

/// Write `text` into `width` bytes, NUL-padded, truncated on a character boundary.
///
/// Truncation is at a boundary rather than at a byte so that a name with a non-ASCII character in
/// it cannot produce invalid UTF-8 in the ring — a reader that has to cope with that will cope with
/// it by guessing.
pub(crate) fn put_text(buf: &mut [u8], at: usize, width: usize, text: &str) {
    let field = &mut buf[at..at + width];
    field.fill(0);
    let mut end = text.len().min(width);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    field[..end].copy_from_slice(&text.as_bytes()[..end]);
}

pub(crate) fn get_text(buf: &[u8], at: usize, width: usize) -> String {
    let field = &buf[at..at + width];
    let end = field.iter().position(|b| *b == 0).unwrap_or(width);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn response_code(response: Option<FaultResponse>) -> u8 {
    match response {
        None => 0,
        Some(FaultResponse::Clamp) => 1,
        Some(FaultResponse::Hold) => 2,
        Some(FaultResponse::StopCat2) => 3,
        Some(FaultResponse::StopCat1) => 4,
        Some(FaultResponse::StopCat0) => 5,
    }
}

fn response_of(code: u8) -> Option<FaultResponse> {
    match code {
        1 => Some(FaultResponse::Clamp),
        2 => Some(FaultResponse::Hold),
        3 => Some(FaultResponse::StopCat2),
        4 => Some(FaultResponse::StopCat1),
        5 => Some(FaultResponse::StopCat0),
        _ => None,
    }
}

/// A guard's wire code: its one-based position in [`GUARDS`].
///
/// The evaluation order is already the sanctioned order (§7.1), so numbering by it means the wire
/// codes and the specification's table cannot drift apart without somebody editing the table.
pub fn guard_code(guard: Guard) -> u8 {
    GUARDS.iter().position(|g| *g == guard).unwrap_or(0) as u8 + 1
}

/// The inverse of [`guard_code`].
pub fn guard_of(code: u8) -> Option<Guard> {
    if code == 0 {
        return None;
    }
    GUARDS.get(code as usize - 1).copied()
}

// ── The four kinds ───────────────────────────────────────────────────────────

/// One control cycle, borrowed from whoever is about to record it.
///
/// The arrays are slices rather than owned vectors because this is the hot-path type: a control
/// loop hands the ring the buffers it already has, and [`crate::ring::Ring::append`] copies them
/// into a slot. Nothing here allocates.
#[derive(Debug, Clone, Copy)]
pub struct CycleView<'a> {
    /// The control tick this cycle is.
    pub tick: u64,
    /// The hash of the observation frame the policy was given (§26: hashes, never bytes).
    pub observation: [u8; 32],
    /// Which policy digest, as an index into the ring header's table.
    pub policy: u16,
    /// `rhal_state.flags`.
    pub state_flags: u16,
    /// `rhal_state.fault_code`, surfaced verbatim (§24).
    pub fault_code: u32,
    /// What `write_command` returned. Negative is an `errno` (§24).
    pub rhal_result: i32,
    /// `rhal_state.seq`.
    pub state_seq: u32,
    /// `rhal_state.stamp_ns` — at acquisition, never at return.
    pub state_stamp_ns: u64,
    /// `rhal_command.stamp_ns` — when the policy produced it.
    pub command_stamp_ns: u64,
    /// `rhal_command.seq`.
    pub command_seq: u32,
    /// A `rhal::Mode` discriminant.
    pub mode: u8,
    /// Whether the tier could only advise (§42): a trip that was logged and forwarded anyway.
    pub advisory: bool,
    /// The strongest response any tripped guard demanded.
    pub response: Option<FaultResponse>,
    /// Guards that tripped, in evaluation order.
    pub tripped: &'a [Guard],
    /// The capability token this command exercised, as an index into the ring header's table.
    ///
    /// [`NO_CAPABILITY`] means the runtime did not say — which is not the same as "no capability",
    /// and the mode gate treats the two differently (§7.2). Recorded because a verdict that cannot
    /// be recomputed is a verdict nobody can check.
    pub capability: u16,
    /// Distance to the nearest detected person, in metres; `NaN` when nothing was measuring it.
    pub nearest_person_m: f32,
    /// `x y z qx qy qz qw`, or all `NaN` when the base is not localised.
    pub base_pose: [f32; 7],
    /// `vx vy vz wx wy wz`.
    pub base_twist: [f32; 6],
    /// What the policy asked for, one per channel.
    pub desired: &'a [f32],
    /// What the kernel decided to send, one per channel.
    pub command: &'a [f64],
    /// The hold mask: 0 means hold this joint (§24).
    pub mask: &'a [u8],
    /// Which channels the kernel reduced.
    pub clamped: &'a [bool],
    /// Measured joint positions.
    pub position: &'a [f32],
    /// Measured velocities; `NaN` where unmeasured.
    pub velocity: &'a [f32],
    /// Measured efforts; `NaN` where unmeasured.
    pub effort: &'a [f32],
}

/// One control cycle, read back out of a ring.
#[derive(Debug, Clone, PartialEq)]
pub struct Cycle {
    /// See [`CycleView::tick`].
    pub tick: u64,
    /// See [`CycleView::observation`].
    pub observation: [u8; 32],
    /// See [`CycleView::policy`].
    pub policy: u16,
    /// See [`CycleView::state_flags`].
    pub state_flags: u16,
    /// See [`CycleView::fault_code`].
    pub fault_code: u32,
    /// See [`CycleView::rhal_result`].
    pub rhal_result: i32,
    /// See [`CycleView::state_seq`].
    pub state_seq: u32,
    /// See [`CycleView::state_stamp_ns`].
    pub state_stamp_ns: u64,
    /// See [`CycleView::command_stamp_ns`].
    pub command_stamp_ns: u64,
    /// See [`CycleView::command_seq`].
    pub command_seq: u32,
    /// See [`CycleView::mode`].
    pub mode: u8,
    /// See [`CycleView::advisory`].
    pub advisory: bool,
    /// See [`CycleView::response`].
    pub response: Option<FaultResponse>,
    /// See [`CycleView::tripped`].
    pub tripped: Vec<Guard>,
    /// See [`CycleView::capability`].
    pub capability: u16,
    /// See [`CycleView::nearest_person_m`].
    pub nearest_person_m: f32,
    /// See [`CycleView::base_pose`].
    pub base_pose: [f32; 7],
    /// See [`CycleView::base_twist`].
    pub base_twist: [f32; 6],
    /// See [`CycleView::desired`].
    pub desired: Vec<f32>,
    /// See [`CycleView::command`].
    pub command: Vec<f64>,
    /// See [`CycleView::mask`].
    pub mask: Vec<u8>,
    /// See [`CycleView::clamped`].
    pub clamped: Vec<bool>,
    /// See [`CycleView::position`].
    pub position: Vec<f32>,
    /// See [`CycleView::velocity`].
    pub velocity: Vec<f32>,
    /// See [`CycleView::effort`].
    pub effort: Vec<f32>,
}

impl Cycle {
    /// Whether the kernel had to intervene this cycle.
    ///
    /// The input to `field_conformance` (§13, §15), and defined as "any guard tripped" rather than
    /// "the command changed". A `Hold` that reproduced the previous command exactly is still an
    /// intervention: the policy asked for something and did not get it, and a conformance ratio
    /// that counted only cycles where the numbers happened to differ would flatter a robot whose
    /// guards fire constantly on a stationary arm.
    pub fn intervened(&self) -> bool {
        !self.tripped.is_empty()
    }
}

/// One assembled observation frame (§25.1).
///
/// Recorded at policy rate rather than joint rate, because that is when assembly happens: the
/// answer to "was the frame the policy saw any good" is per frame, and per cycle it would be the
/// same answer repeated thirty times.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The hash of the assembled frame — the same value a [`Cycle`] carries.
    pub observation: [u8; 32],
    /// The instant the assembler targeted.
    pub target_ns: u64,
    /// The worst skew between any stream in the frame and that instant.
    pub worst_skew_us: u32,
    /// Streams the frame could not fit; reported so the record is not silently partial.
    pub streams_omitted: u16,
    /// Per stream: how far back its sample was, and whether that broke its budget.
    pub streams: Vec<Stream>,
}

/// One stream's contribution to a [`Frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// The stream's name, truncated to [`STREAM_NAME`] bytes.
    pub name: String,
    /// How old this stream's sample was, relative to the frame's target instant.
    pub age_us: u32,
    /// Whether that exceeded the policy's declared staleness budget — which is a `hold`, not a
    /// silently older frame (§25.1).
    pub stale: bool,
}

/// Something that changed (§26's `/sk/event`).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// What happened.
    pub code: EventCode,
    /// Who it happened to: a package id, a capability token, an image slot.
    pub subject: String,
    /// A sentence about it, for a human reading the sealed file.
    pub detail: String,
}

/// The event vocabulary.
///
/// A closed enum rather than a free string, because §26 lists what belongs on this channel and an
/// open vocabulary is how ordinary logging ends up in the black box — which is precisely what §41.1
/// forbids, and what would break the completeness guarantee by filling the ring with prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EventCode {
    /// A capability lease was granted (§25.2).
    LeaseGranted = 1,
    /// A lease was released by its holder.
    LeaseReleased = 2,
    /// A safety behaviour pre-empted a lease — the one allowed pre-emption (§25.2).
    LeasePreempted = 3,
    /// An emergency stop was asserted.
    Estop = 4,
    /// An emergency stop was cleared.
    EstopCleared = 5,
    /// A package started.
    PackageStarted = 6,
    /// A package stopped, with its exit status in the detail (§33.5).
    PackageStopped = 7,
    /// A capability grant was widened or narrowed (§38).
    GrantChanged = 8,
    /// An OS image was staged, applied or rolled back (§12).
    Ota = 9,
    /// `CLOCK_REALTIME` stepped. Recorded so a wall-time jump is visible in replay rather than
    /// being a mystery in the timeline (§35).
    ClockStep = 10,
    /// The recording began.
    RecordingStarted = 11,
    /// The recording was sealed.
    Sealed = 12,
    /// A configuration change (§38.3): a key trusted or untrusted, a profile applied.
    ///
    /// **Mirrored rather than written directly.** The change is usually made by a *different
    /// process* — `skynetd trust add` runs while the daemon is up — and this ring has exactly one
    /// writer by construction. So the change goes into the audit log, and the daemon copies
    /// anything past its cursor onto this channel on its next pass. The consequence is a delay of
    /// up to one reconcile period, and none at all on a robot where `skynetd` is not running; the
    /// audit entry is the record that always exists, and this one is the one an incident replay
    /// can see beside the cycles it was reading anyway.
    ConfigChanged = 13,
    /// A package was admitted, or was not (§8, §37.1).
    ///
    /// The verdict rather than the whole report: which package, which digest, and what admission
    /// concluded. §41.1 keeps the black box out of the logging business, so the findings stay in
    /// the report an operator reads and what lands here is the decision.
    Admission = 14,
}

impl EventCode {
    /// Decode a wire discriminant.
    pub fn from_u16(value: u16) -> Option<EventCode> {
        Some(match value {
            1 => EventCode::LeaseGranted,
            2 => EventCode::LeaseReleased,
            3 => EventCode::LeasePreempted,
            4 => EventCode::Estop,
            5 => EventCode::EstopCleared,
            6 => EventCode::PackageStarted,
            7 => EventCode::PackageStopped,
            8 => EventCode::GrantChanged,
            9 => EventCode::Ota,
            10 => EventCode::ClockStep,
            11 => EventCode::RecordingStarted,
            12 => EventCode::Sealed,
            13 => EventCode::ConfigChanged,
            14 => EventCode::Admission,
            _ => return None,
        })
    }

    /// The wire spelling, for the sealed file and the console.
    pub fn as_str(self) -> &'static str {
        match self {
            EventCode::LeaseGranted => "lease_granted",
            EventCode::LeaseReleased => "lease_released",
            EventCode::LeasePreempted => "lease_preempted",
            EventCode::Estop => "estop",
            EventCode::EstopCleared => "estop_cleared",
            EventCode::PackageStarted => "package_started",
            EventCode::PackageStopped => "package_stopped",
            EventCode::GrantChanged => "grant_changed",
            EventCode::Ota => "ota",
            EventCode::ClockStep => "clock_step",
            EventCode::RecordingStarted => "recording_started",
            EventCode::Sealed => "sealed",
            EventCode::ConfigChanged => "config_changed",
            EventCode::Admission => "admission",
        }
    }
}

/// Records that should exist and do not (§47.4).
///
/// > If the encoder falls behind, recording frames are dropped **and the drop is recorded** — a gap
/// > in the black box that is annotated as a gap is honest; a gap that looks like a quiet period is
/// > not.
///
/// A producer that had to drop writes one of these when it recovers. It consumes a sequence number
/// of its own, so the numbering stays dense and a reader can distinguish "the ring wrapped past
/// these" from "these were never written".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// How many records were dropped.
    pub dropped: u64,
    /// The first tick that went unrecorded, so the hole can be placed on a timeline.
    pub first_tick: u64,
    /// Why, as a code the console can name.
    pub reason: GapReason,
}

/// Why records were dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum GapReason {
    /// The writer could not keep up with the cycle rate.
    WriterBehind = 1,
    /// A sensor pipeline dropped frames before the recorder saw them (§47.4).
    EncoderBehind = 2,
    /// The recorder was not running — a restart, a late start.
    NotRecording = 3,
    /// A write returned an error.
    WriteFailed = 4,
}

impl GapReason {
    /// Decode a wire discriminant.
    pub fn from_u16(value: u16) -> Option<GapReason> {
        Some(match value {
            1 => GapReason::WriterBehind,
            2 => GapReason::EncoderBehind,
            3 => GapReason::NotRecording,
            4 => GapReason::WriteFailed,
            _ => return None,
        })
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            GapReason::WriterBehind => "writer_behind",
            GapReason::EncoderBehind => "encoder_behind",
            GapReason::NotRecording => "not_recording",
            GapReason::WriteFailed => "write_failed",
        }
    }
}

/// One decoded record, with its prefix.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The record's place in the recording. Dense, from one, and never reused.
    pub sequence: u64,
    /// `CLOCK_MONOTONIC_RAW` when the record was produced (§35.1).
    pub mono_ns: u64,
    /// What it holds.
    pub body: Body,
}

/// The kind-specific half of a [`Record`].
///
/// A [`Cycle`] is several times the size of the other three, and clippy is right that the enum is
/// therefore as large as its largest variant. Boxing it is the usual fix and is the wrong one here:
/// cycles are what a recording is almost entirely made of, so boxing would put an allocation on the
/// common path to save a few dozen bytes on the rare one — and this type is produced while reading a
/// recording, where the ring's fixed slot has already decided the memory.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// See [`Cycle`].
    Cycle(Cycle),
    /// See [`Frame`].
    Frame(Frame),
    /// See [`Event`].
    Event(Event),
    /// See [`Gap`].
    Gap(Gap),
}

impl Body {
    /// Which kind this is.
    pub fn kind(&self) -> Kind {
        match self {
            Body::Cycle(_) => Kind::Cycle,
            Body::Frame(_) => Kind::Frame,
            Body::Event(_) => Kind::Event,
            Body::Gap(_) => Kind::Gap,
        }
    }
}

/// What a producer hands [`crate::ring::Ring::append`].
///
/// Borrowed throughout, which is the same asymmetry the hot path has: a cycle happens a thousand
/// times a second and must not allocate, and the rare kinds are borrowed too because there is no
/// reason for them not to be.
#[derive(Debug, Clone, Copy)]
pub enum Entry<'a> {
    /// See [`CycleView`].
    Cycle(&'a CycleView<'a>),
    /// See [`FrameView`].
    Frame(&'a FrameView<'a>),
    /// See [`EventView`].
    Event(&'a EventView<'a>),
    /// See [`Gap`].
    Gap(Gap),
}

/// A [`Frame`] being written, borrowing its stream names.
#[derive(Debug, Clone, Copy)]
pub struct FrameView<'a> {
    /// See [`Frame::observation`].
    pub observation: [u8; 32],
    /// See [`Frame::target_ns`].
    pub target_ns: u64,
    /// See [`Frame::worst_skew_us`].
    pub worst_skew_us: u32,
    /// Streams beyond [`MAX_STREAMS`], counted rather than written.
    pub streams_omitted: u16,
    /// Per stream: name, age, and whether the budget broke.
    pub streams: &'a [(&'a str, u32, bool)],
}

/// An [`Event`] being written, borrowing its text.
#[derive(Debug, Clone, Copy)]
pub struct EventView<'a> {
    /// See [`Event::code`].
    pub code: EventCode,
    /// See [`Event::subject`].
    pub subject: &'a str,
    /// See [`Event::detail`].
    pub detail: &'a str,
}

impl Entry<'_> {
    /// Which kind this entry will be written as.
    pub fn kind(&self) -> Kind {
        match self {
            Entry::Cycle(_) => Kind::Cycle,
            Entry::Frame(_) => Kind::Frame,
            Entry::Event(_) => Kind::Event,
            Entry::Gap(_) => Kind::Gap,
        }
    }
}

/// Why a record could not be written or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// A cycle's arrays did not match the shape the ring was created with.
    ///
    /// Refused rather than truncated or padded. A ring whose records are the wrong length is a ring
    /// whose every subsequent read is misaligned, and a recorder that silently accepted a shorter
    /// array would be recording a different robot from the one in the header.
    Shape {
        /// Which array.
        field: &'static str,
        /// How long the ring's geometry says it is.
        expected: usize,
        /// How long the caller's was.
        found: usize,
    },
    /// The slot's CRC does not match its contents: a torn write, or a damaged disk.
    Torn,
    /// The slot has never been written.
    Empty,
    /// The slot's kind byte is not one this edition knows.
    UnknownKind(u8),
    /// The slot is shorter than the record it claims to hold.
    Truncated,
}

/// Encode `entry` into `slot`, which must be at least [`Shape::record_size`] bytes.
///
/// Zeroes the slot first. That costs a memset per record and buys something worth more: a slot
/// re-used after a wrap holds no bytes of the record it replaced, so a reader looking at a hex dump
/// is never reading two records superimposed, and a shorter kind written over a longer one cannot
/// leave a plausible tail behind it.
pub fn encode(
    shape: Shape,
    slot: &mut [u8],
    sequence: u64,
    mono_ns: u64,
    entry: Entry<'_>,
) -> Result<(), RecordError> {
    let size = shape.record_size();
    if slot.len() < size {
        return Err(RecordError::Truncated);
    }
    let slot = &mut slot[..size];
    slot.fill(0);

    slot[4] = entry.kind() as u8;
    // The record schema version, per record rather than per file: a ring outlives a daemon upgrade,
    // and a reader must be able to tell which layout a given slot is in.
    slot[5] = 1;
    put_u64(slot, 8, sequence);
    put_u64(slot, 16, mono_ns);

    match entry {
        Entry::Cycle(cycle) => encode_cycle(shape, slot, cycle)?,
        Entry::Frame(frame) => encode_frame(slot, frame),
        Entry::Event(event) => encode_event(slot, event),
        Entry::Gap(gap) => {
            put_u64(slot, PREFIX, gap.dropped);
            put_u64(slot, PREFIX + 8, gap.first_tick);
            put_u16(slot, PREFIX + 16, gap.reason as u16);
        }
    }

    let crc = crate::crc32::crc32(&slot[4..]);
    put_u32(slot, 0, crc);
    Ok(())
}

fn encode_cycle(shape: Shape, slot: &mut [u8], cycle: &CycleView<'_>) -> Result<(), RecordError> {
    let channels = shape.channels as usize;
    let joints = shape.joints as usize;
    let check = |field: &'static str, found: usize, expected: usize| {
        if found == expected {
            Ok(())
        } else {
            Err(RecordError::Shape {
                field,
                expected,
                found,
            })
        }
    };
    check("desired", cycle.desired.len(), channels)?;
    check("command", cycle.command.len(), channels)?;
    check("mask", cycle.mask.len(), channels)?;
    check("clamped", cycle.clamped.len(), channels)?;
    check("position", cycle.position.len(), joints)?;
    check("velocity", cycle.velocity.len(), joints)?;
    check("effort", cycle.effort.len(), joints)?;

    put_u64(slot, 24, cycle.tick);
    slot[32..64].copy_from_slice(&cycle.observation);
    put_u16(slot, 64, cycle.policy);
    put_u16(slot, 66, cycle.state_flags);
    put_u32(slot, 68, cycle.fault_code);
    put_u32(slot, 72, cycle.rhal_result as u32);
    put_u32(slot, 76, cycle.state_seq);
    put_u64(slot, 80, cycle.state_stamp_ns);
    put_u64(slot, 88, cycle.command_stamp_ns);
    put_u32(slot, 96, cycle.command_seq);
    slot[100] = cycle.mode;
    slot[101] = u8::from(cycle.advisory);
    slot[102] = response_code(cycle.response);
    slot[103] = cycle.tripped.len().min(MAX_TRIPPED) as u8;
    for (index, guard) in cycle.tripped.iter().take(MAX_TRIPPED).enumerate() {
        slot[104 + index] = guard_code(*guard);
    }
    put_u16(slot, 112, cycle.capability);
    put_f32(slot, 116, cycle.nearest_person_m);
    for (index, value) in cycle.base_pose.iter().enumerate() {
        put_f32(slot, 120 + index * 4, *value);
    }
    for (index, value) in cycle.base_twist.iter().enumerate() {
        put_f32(slot, 148 + index * 4, *value);
    }

    let at = shape.cycle_offsets();
    for (index, value) in cycle.desired.iter().enumerate() {
        put_f32(slot, at.desired + index * 4, *value);
    }
    for (index, value) in cycle.command.iter().enumerate() {
        put_f64(slot, at.command + index * 8, *value);
    }
    slot[at.mask..at.mask + channels].copy_from_slice(cycle.mask);
    for (index, clamped) in cycle.clamped.iter().enumerate() {
        if *clamped {
            slot[at.clamped + index / 8] |= 1 << (index % 8);
        }
    }
    for (index, value) in cycle.position.iter().enumerate() {
        put_f32(slot, at.position + index * 4, *value);
    }
    for (index, value) in cycle.velocity.iter().enumerate() {
        put_f32(slot, at.velocity + index * 4, *value);
    }
    for (index, value) in cycle.effort.iter().enumerate() {
        put_f32(slot, at.effort + index * 4, *value);
    }
    Ok(())
}

fn encode_frame(slot: &mut [u8], frame: &FrameView<'_>) {
    slot[PREFIX..PREFIX + 32].copy_from_slice(&frame.observation);
    put_u64(slot, PREFIX + 32, frame.target_ns);
    put_u32(slot, PREFIX + 40, frame.worst_skew_us);
    let written = frame.streams.len().min(MAX_STREAMS);
    put_u16(slot, PREFIX + 44, written as u16);
    // Anything the caller could not fit, plus anything this record could not: a frame with eight
    // streams records six and says two are missing, whoever noticed first.
    let overflow = frame.streams.len().saturating_sub(MAX_STREAMS) as u16;
    put_u16(
        slot,
        PREFIX + 46,
        frame.streams_omitted.saturating_add(overflow),
    );
    let base = PREFIX + 48;
    for (index, (name, age_us, stale)) in frame.streams.iter().take(MAX_STREAMS).enumerate() {
        let at = base + index * (STREAM_NAME + 8);
        put_text(slot, at, STREAM_NAME, name);
        put_u32(slot, at + STREAM_NAME, *age_us);
        slot[at + STREAM_NAME + 4] = u8::from(*stale);
    }
}

fn encode_event(slot: &mut [u8], event: &EventView<'_>) {
    put_u16(slot, PREFIX, event.code as u16);
    put_text(slot, PREFIX + 4, EVENT_SUBJECT, event.subject);
    put_text(slot, PREFIX + 4 + EVENT_SUBJECT, EVENT_DETAIL, event.detail);
}

/// Decode one slot.
///
/// The CRC is checked before anything is interpreted, so a torn slot is reported as torn rather
/// than as a cycle with impossible numbers in it.
pub fn decode(shape: Shape, slot: &[u8]) -> Result<Record, RecordError> {
    let size = shape.record_size();
    if slot.len() < size {
        return Err(RecordError::Truncated);
    }
    let slot = &slot[..size];
    if slot.iter().all(|byte| *byte == 0) {
        return Err(RecordError::Empty);
    }
    if get_u32(slot, 0) != crate::crc32::crc32(&slot[4..]) {
        return Err(RecordError::Torn);
    }
    let Some(kind) = Kind::from_u8(slot[4]) else {
        return Err(RecordError::UnknownKind(slot[4]));
    };
    let body = match kind {
        Kind::Cycle => Body::Cycle(decode_cycle(shape, slot)),
        Kind::Frame => Body::Frame(decode_frame(slot)),
        Kind::Event => Body::Event(decode_event(slot)?),
        Kind::Gap => Body::Gap(Gap {
            dropped: get_u64(slot, PREFIX),
            first_tick: get_u64(slot, PREFIX + 8),
            reason: GapReason::from_u16(get_u16(slot, PREFIX + 16))
                .ok_or(RecordError::UnknownKind(slot[PREFIX + 16]))?,
        }),
    };
    Ok(Record {
        sequence: get_u64(slot, 8),
        mono_ns: get_u64(slot, 16),
        body,
    })
}

fn decode_cycle(shape: Shape, slot: &[u8]) -> Cycle {
    let channels = shape.channels as usize;
    let joints = shape.joints as usize;
    let at = shape.cycle_offsets();

    let mut observation = [0u8; 32];
    observation.copy_from_slice(&slot[32..64]);

    let mut base_pose = [0f32; 7];
    for (index, value) in base_pose.iter_mut().enumerate() {
        *value = get_f32(slot, 120 + index * 4);
    }
    let mut base_twist = [0f32; 6];
    for (index, value) in base_twist.iter_mut().enumerate() {
        *value = get_f32(slot, 148 + index * 4);
    }

    let tripped_count = (slot[103] as usize).min(MAX_TRIPPED);
    let mut tripped = Vec::with_capacity(tripped_count);
    for index in 0..tripped_count {
        if let Some(guard) = guard_of(slot[104 + index]) {
            tripped.push(guard);
        }
    }

    let mut desired = vec![0f32; channels];
    for (index, value) in desired.iter_mut().enumerate() {
        *value = get_f32(slot, at.desired + index * 4);
    }
    let mut command = vec![0f64; channels];
    for (index, value) in command.iter_mut().enumerate() {
        *value = get_f64(slot, at.command + index * 8);
    }
    let mask = slot[at.mask..at.mask + channels].to_vec();
    let mut clamped = vec![false; channels];
    for (index, value) in clamped.iter_mut().enumerate() {
        *value = slot[at.clamped + index / 8] & (1 << (index % 8)) != 0;
    }
    let read = |base: usize| {
        let mut values = vec![0f32; joints];
        for (index, value) in values.iter_mut().enumerate() {
            *value = get_f32(slot, base + index * 4);
        }
        values
    };

    Cycle {
        tick: get_u64(slot, 24),
        observation,
        policy: get_u16(slot, 64),
        state_flags: get_u16(slot, 66),
        fault_code: get_u32(slot, 68),
        rhal_result: get_u32(slot, 72) as i32,
        state_seq: get_u32(slot, 76),
        state_stamp_ns: get_u64(slot, 80),
        command_stamp_ns: get_u64(slot, 88),
        command_seq: get_u32(slot, 96),
        mode: slot[100],
        advisory: slot[101] != 0,
        response: response_of(slot[102]),
        tripped,
        capability: get_u16(slot, 112),
        nearest_person_m: get_f32(slot, 116),
        base_pose,
        base_twist,
        desired,
        command,
        mask,
        clamped,
        position: read(at.position),
        velocity: read(at.velocity),
        effort: read(at.effort),
    }
}

fn decode_frame(slot: &[u8]) -> Frame {
    let mut observation = [0u8; 32];
    observation.copy_from_slice(&slot[PREFIX..PREFIX + 32]);
    let written = (get_u16(slot, PREFIX + 44) as usize).min(MAX_STREAMS);
    let base = PREFIX + 48;
    let mut streams = Vec::with_capacity(written);
    for index in 0..written {
        let at = base + index * (STREAM_NAME + 8);
        streams.push(Stream {
            name: get_text(slot, at, STREAM_NAME),
            age_us: get_u32(slot, at + STREAM_NAME),
            stale: slot[at + STREAM_NAME + 4] != 0,
        });
    }
    Frame {
        observation,
        target_ns: get_u64(slot, PREFIX + 32),
        worst_skew_us: get_u32(slot, PREFIX + 40),
        streams_omitted: get_u16(slot, PREFIX + 46),
        streams,
    }
}

fn decode_event(slot: &[u8]) -> Result<Event, RecordError> {
    let raw = get_u16(slot, PREFIX);
    let Some(code) = EventCode::from_u16(raw) else {
        return Err(RecordError::UnknownKind(slot[PREFIX]));
    };
    Ok(Event {
        code,
        subject: get_text(slot, PREFIX + 4, EVENT_SUBJECT),
        detail: get_text(slot, PREFIX + 4 + EVENT_SUBJECT, EVENT_DETAIL),
    })
}
