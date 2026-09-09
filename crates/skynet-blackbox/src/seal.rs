//! Sealing: the ring becomes a file (§13, §26, §56.4).
//!
//! > Sealed and uploaded on any fault, estop, or operator request.
//!
//! Sealing walks the ring in sequence order, fans each record onto the channels §26 gives it, and
//! produces an MCAP file plus the two things anyone receiving it needs to know before reading a
//! single message:
//!
//! * **its digest**, which is §56.4's "black box segment hash" — the value an evaluation bundle
//!   carries, so a disputed benchmark result names the recording that would settle it;
//! * **its [`Continuity`]**, which is what `telemetry.blackbox.complete` asks about: how many
//!   records, over which sequence numbers, with which holes, and how many slots were damaged.
//!
//! # Completeness is reported, never asserted
//!
//! §41.1 makes the black box's guarantee "complete, or the run is not conformant", and the
//! temptation is for `seal` to refuse when it is not. It does not, and the reason matters: the
//! recording most likely to have a hole in it is the one covering the fault that made the hole.
//! Refusing to produce it would destroy the only evidence at exactly the moment it is worth
//! something.
//!
//! So the file is always produced and the truth travels with it. [`Continuity::complete`] answers
//! the conformance question, and the answer is in the file's own metadata as well as in the return
//! value, so a recording separated from this API still says what it is.
//!
//! # One cycle becomes four messages
//!
//! §26's table splits a control cycle across `/sk/state`, `/sk/desired`, `/sk/verdict` and
//! `/sk/command`, and the ring holds it as one record. That is not a redundancy to collapse: the
//! ring is optimised for writing a thousand times a second, and the file is optimised for somebody
//! asking a question. "What did the hardware report" and "what did the policy want" are different
//! questions, and in Foxglove they are different panels.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;
use skynet_sha256::digest;

use crate::mcap::{hex, Writer};
use crate::record::{Body, Cycle, Event, Frame, Gap, ReadOutcome, Record, NO_CAPABILITY};
use crate::ring::Ring;
use crate::schema::{self, id};
use crate::LIBRARY;

/// Why a recording was sealed.
///
/// §13 lists three; two more exist because the code has to name them, and naming them separately is
/// the difference between "this file covers an incident" and "this file covers a benchmark run".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A guard tripped hard enough to stop the robot, or a component faulted.
    Fault,
    /// An emergency stop was asserted.
    Estop,
    /// Somebody asked, through the console or the CLI.
    Operator,
    /// An on-robot evaluation run finished, and the bundle needs a segment to point at (§56.4).
    Evaluation,
    /// The robot is shutting down and the window would otherwise be lost.
    Shutdown,
}

impl Reason {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Fault => "fault",
            Reason::Estop => "estop",
            Reason::Operator => "operator",
            Reason::Evaluation => "evaluation",
            Reason::Shutdown => "shutdown",
        }
    }
}

/// A run of sequence numbers the recording does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hole {
    /// The first sequence number missing.
    pub from: u64,
    /// The last sequence number missing.
    pub to: u64,
}

impl Hole {
    /// How many records are missing.
    pub fn len(&self) -> u64 {
        self.to - self.from + 1
    }

    /// Whether the hole is empty, which it never is — present because clippy asks for it beside
    /// [`Hole::len`], and a hole with no records in it would not have been recorded.
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// What the recording holds, and what it does not.
///
/// The answer to `telemetry.blackbox.complete`, in the form that makes the answer checkable rather
/// than assertable.
#[derive(Debug, Clone, PartialEq)]
pub struct Continuity {
    /// How many records the window holds.
    pub records: usize,
    /// The lowest sequence number present.
    pub first: u64,
    /// The highest.
    pub last: u64,
    /// Sequence numbers between [`Continuity::first`] and [`Continuity::last`] that are absent.
    ///
    /// A hole here is a hole *inside* the window, which is a defect. Records older than `first` are
    /// not holes: the ring is a window, and the window moving on is what a ring is for.
    pub holes: Vec<Hole>,
    /// Slots whose CRC did not match — torn writes, or a damaged disk.
    pub damaged: usize,
    /// Records the producer said it dropped, summed over every [`Gap`] in the window (§47.4).
    pub dropped: u64,
    /// How many cycles are in the window.
    pub cycles: u64,
    /// How many of those the kernel intervened in.
    ///
    /// The numerator of `field_conformance` (§13, §15). This crate reports the two counts and does
    /// not compute the ratio, because the ratio is already defined once, in
    /// `skynet_contracts::certification::field_conformance`, including the part that matters — a
    /// window of zero cycles is `undefined`, never a flattering 1.0.
    pub interventions: u64,
}

impl Continuity {
    /// Whether the window is unbroken: no holes, nothing damaged, nothing dropped.
    pub fn complete(&self) -> bool {
        self.holes.is_empty() && self.damaged == 0 && self.dropped == 0
    }
}

/// A sealed recording.
#[derive(Debug, Clone)]
pub struct Sealed {
    /// The MCAP file.
    pub bytes: Vec<u8>,
    /// SHA-256 of [`Sealed::bytes`] — §56.4's black box segment hash.
    pub digest: [u8; 32],
    /// Why it was sealed.
    pub reason: Reason,
    /// What it holds.
    pub continuity: Continuity,
}

/// Walk a ring and produce a file.
///
/// `mono_ns` is the instant of the seal, recorded as the closing event, so a reader can see how
/// long after the last cycle the decision to seal was taken. On a fault that interval is the
/// robot's reaction time, which is worth having.
pub fn seal(ring: &Ring, store: &[u8], reason: Reason, mono_ns: u64) -> Sealed {
    let held = ring.ordered(store);
    let damaged = ring
        .records(store)
        .filter(|(_, record)| matches!(record, Err(crate::record::RecordError::Torn)))
        .count();
    let continuity = continuity_of(&held, damaged);
    let meta = ring.meta();

    let mut writer = Writer::new(LIBRARY);
    // Every schema, then every channel, rather than each pair in turn. Both are legal — MCAP only
    // asks that a schema precede the channel referring to it — and the grouped order is the one the
    // summary section has to use anyway, so the data section reading the same way means a person
    // comparing the two halves of a file is comparing like with like.
    let wire = schema::wire();
    for entry in &wire {
        writer.schema(entry.schema.clone());
    }
    for entry in &wire {
        writer.channel(entry.channel);
    }

    // The provenance §56.2 asks a bundle to carry, in the file rather than beside it. A recording
    // that arrives without its embodiment digest cannot be replayed against anything, and one that
    // arrives without its simulator flag can be mistaken for a field result.
    let mut pairs: Vec<(String, String)> = Vec::new();
    pairs.push(("robot_id".to_string(), meta.robot_id.clone()));
    pairs.push(("embodiment_digest".to_string(), hex(&meta.embodiment)));
    pairs.push(("os_image_digest".to_string(), hex(&meta.os_image)));
    pairs.push(("tier".to_string(), meta.tier.to_string()));
    pairs.push(("enforcement".to_string(), meta.enforcement.to_string()));
    pairs.push((
        "simulator".to_string(),
        if meta.simulator { "true" } else { "false" }.to_string(),
    ));
    pairs.push(("sealed_reason".to_string(), reason.as_str().to_string()));
    pairs.push(("sealed_mono_ns".to_string(), mono_ns.to_string()));
    pairs.push(("wall_epoch_ns".to_string(), meta.wall_epoch_ns.to_string()));
    pairs.push(("mono_ns_at_open".to_string(), meta.mono_ns.to_string()));
    pairs.push(("records".to_string(), continuity.records.to_string()));
    pairs.push(("first_sequence".to_string(), continuity.first.to_string()));
    pairs.push(("last_sequence".to_string(), continuity.last.to_string()));
    pairs.push(("damaged_slots".to_string(), continuity.damaged.to_string()));
    pairs.push((
        "dropped_records".to_string(),
        continuity.dropped.to_string(),
    ));
    pairs.push((
        "holes".to_string(),
        continuity
            .holes
            .iter()
            .map(|hole| format!("{}-{}", hole.from, hole.to))
            .collect::<Vec<_>>()
            .join(","),
    ));
    pairs.push((
        "complete".to_string(),
        if continuity.complete() {
            "true"
        } else {
            "false"
        }
        .to_string(),
    ));
    pairs.push(("cycles".to_string(), continuity.cycles.to_string()));
    pairs.push((
        "interventions".to_string(),
        continuity.interventions.to_string(),
    ));
    for (index, digest) in ring.digests().iter().enumerate() {
        pairs.push((format!("policy_digest_{index}"), hex(digest)));
    }
    writer.metadata("skynet.blackbox", &pairs);

    let channels = &meta.channels;
    let digests = ring.digests();
    let capabilities = ring.capabilities();
    for record in &held {
        let log_time = wall_of(meta.wall_epoch_ns, meta.mono_ns, record.mono_ns);
        match &record.body {
            Body::Cycle(cycle) => {
                writer.message(id::STATE, log_time, &state_message(record, cycle));
                writer.message(
                    id::DESIRED,
                    log_time,
                    &desired_message(record, cycle, channels, digests, capabilities),
                );
                writer.message(
                    id::VERDICT,
                    log_time,
                    &verdict_message(record, cycle, channels),
                );
                writer.message(
                    id::COMMAND,
                    log_time,
                    &command_message(record, cycle, channels),
                );
            }
            Body::Frame(frame) => {
                writer.message(id::FRAME, log_time, &frame_message(record, frame));
            }
            Body::Event(event) => {
                writer.message(id::EVENT, log_time, &event_message(record, event));
            }
            Body::Gap(gap) => {
                writer.message(id::EVENT, log_time, &gap_message(record, gap));
            }
        }
    }

    // The seal is itself an event, on the same channel as everything else that changed. A file
    // whose last message is the reason it exists reads correctly from either end.
    let mut sealing = Json::obj();
    sealing.set("mono_ns", Json::Num(mono_ns as f64));
    sealing.set("event", Json::Str("sealed".into()));
    sealing.set("subject", Json::Str(reason.as_str().into()));
    sealing.set(
        "detail",
        Json::Str(format!(
            "{} records, sequences {}-{}, {} damaged, {} dropped, {}",
            continuity.records,
            continuity.first,
            continuity.last,
            continuity.damaged,
            continuity.dropped,
            if continuity.complete() {
                "complete"
            } else {
                "incomplete"
            }
        )),
    );
    writer.message(
        id::EVENT,
        wall_of(meta.wall_epoch_ns, meta.mono_ns, mono_ns),
        &canonical(&sealing),
    );

    let bytes = writer.finish();
    let digest = digest(&bytes);
    Sealed {
        bytes,
        digest,
        reason,
        continuity,
    }
}

/// Where a recording's monotonic instant lands on a wall clock.
///
/// One correlation, taken when the ring was opened, applied to every record. Not a per-record wall
/// clock read: §35.1 gives `CLOCK_REALTIME` exactly one job, and a timeline assembled from a clock
/// that can step is a timeline in which the robot occasionally acts before it is commanded.
///
/// Saturating rather than wrapping, because a ring opened before the wall clock was set — which is
/// every robot with no RTC, on every boot before NTP answers — would otherwise produce timestamps
/// in the year 2554.
fn wall_of(wall_epoch_ns: u64, mono_at_open: u64, mono_ns: u64) -> u64 {
    wall_epoch_ns.saturating_add(mono_ns.saturating_sub(mono_at_open))
}

fn continuity_of(held: &[Record], damaged: usize) -> Continuity {
    let first = held.first().map(|record| record.sequence).unwrap_or(0);
    let last = held.last().map(|record| record.sequence).unwrap_or(0);
    let mut holes = Vec::new();
    for pair in held.windows(2) {
        let (before, after) = (pair[0].sequence, pair[1].sequence);
        if after > before + 1 {
            holes.push(Hole {
                from: before + 1,
                to: after - 1,
            });
        }
    }
    let dropped = held
        .iter()
        .filter_map(|record| match &record.body {
            Body::Gap(gap) => Some(gap.dropped),
            _ => None,
        })
        .sum();
    let cycles = held
        .iter()
        .filter(|record| matches!(record.body, Body::Cycle(_)))
        .count() as u64;
    let interventions = held
        .iter()
        .filter(|record| match &record.body {
            Body::Cycle(cycle) => cycle.intervened(),
            _ => false,
        })
        .count() as u64;
    Continuity {
        records: held.len(),
        first,
        last,
        holes,
        damaged,
        dropped,
        cycles,
        interventions,
    }
}

/// A number, or `null` where a sensor said `NaN` (§24).
fn number(value: f64) -> Json {
    if value.is_finite() {
        Json::Num(value)
    } else {
        Json::Null
    }
}

fn numbers(values: &[f32]) -> Json {
    let mut array = Json::arr();
    for value in values {
        array.push(number(*value as f64));
    }
    array
}

fn canonical(document: &Json) -> String {
    document
        .to_canonical()
        .unwrap_or_else(|_| String::from("{}"))
}

fn mode_name(mode: u8) -> &'static str {
    // Mirrors `skynet_rhal::Mode`, which this crate deliberately does not depend on — see the
    // manifest. An unknown mode is named as unknown rather than defaulted to position: a recording
    // that quietly relabelled a torque command as a position command would be worse than one that
    // said it did not know.
    match mode {
        0 => "position",
        1 => "velocity",
        2 => "torque",
        _ => "unknown",
    }
}

fn channel_name(channels: &[String], index: usize) -> String {
    channels
        .get(index)
        .cloned()
        .unwrap_or_else(|| format!("channel_{index}"))
}

fn values_array(channels: &[String], values: &[f64], mask: &[u8]) -> Json {
    let mut array = Json::arr();
    for (index, value) in values.iter().enumerate() {
        let mut item = Json::obj();
        item.set("channel", Json::Str(channel_name(channels, index)));
        item.set("value", number(*value));
        item.set(
            "hold",
            Json::Bool(mask.get(index).map(|byte| *byte == 0).unwrap_or(false)),
        );
        array.push(item);
    }
    array
}

fn state_message(record: &Record, cycle: &Cycle) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("stamp_ns", Json::Num(cycle.state_stamp_ns as f64));
    document.set("seq", Json::Num(cycle.state_seq as f64));
    document.set("flags", Json::Num(cycle.state_flags as f64));
    document.set("fault_code", Json::Num(cycle.fault_code as f64));
    // `null` is *unrecorded*, not *succeeded* — the distinction §26.4 exists to keep, carried into
    // the file so a reader of a schema-1 window is told it cannot know rather than told the read
    // was fine.
    document.set(
        "read_result",
        match cycle.read {
            ReadOutcome::Unrecorded => Json::Null,
            ReadOutcome::Ok => Json::Num(0.0),
            ReadOutcome::Failed(errno) => Json::Num(errno as f64),
        },
    );
    document.set("nearest_person_m", number(cycle.nearest_person_m as f64));
    document.set("position_rad", numbers(&cycle.position));
    document.set("velocity_rad_s", numbers(&cycle.velocity));
    document.set("effort_nm", numbers(&cycle.effort));
    document.set("base_pose", numbers(&cycle.base_pose));
    document.set("base_twist", numbers(&cycle.base_twist));
    canonical(&document)
}

fn desired_message(
    record: &Record,
    cycle: &Cycle,
    channels: &[String],
    digests: &[[u8; 32]],
    capabilities: &[String],
) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("tick", Json::Num(cycle.tick as f64));
    document.set("stamp_ns", Json::Num(cycle.command_stamp_ns as f64));
    document.set("seq", Json::Num(cycle.command_seq as f64));
    document.set("mode", Json::Str(mode_name(cycle.mode).into()));
    document.set(
        "policy_digest",
        Json::Str(
            digests
                .get(cycle.policy as usize)
                .map(|digest| hex(digest))
                .unwrap_or_default(),
        ),
    );
    // The token rather than the index, for `policy_digest`'s reason one field up: a sealed window
    // travels away from the ring that holds the table, and an index into a table nobody has is not
    // a fact. `NO_CAPABILITY` is `null`, which is *the runtime did not say* rather than *no
    // capability* — §7.2 treats the two differently and so must the file.
    document.set(
        "capability",
        if cycle.capability == NO_CAPABILITY {
            Json::Null
        } else {
            match capabilities.get(cycle.capability as usize) {
                Some(token) => Json::Str(token.clone()),
                None => Json::Null,
            }
        },
    );
    document.set("observation", Json::Str(hex(&cycle.observation)));
    let desired: Vec<f64> = cycle.desired.iter().map(|value| *value as f64).collect();
    document.set("values", values_array(channels, &desired, &cycle.mask));
    canonical(&document)
}

fn verdict_message(record: &Record, cycle: &Cycle, channels: &[String]) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("tick", Json::Num(cycle.tick as f64));
    document.set(
        "response",
        match cycle.response {
            Some(response) => Json::Str(response.as_str().into()),
            None => Json::Null,
        },
    );
    let mut tripped = Json::arr();
    for guard in &cycle.tripped {
        tripped.push(Json::Str(guard.as_str().into()));
    }
    document.set("tripped", tripped);
    let mut clamped = Json::arr();
    for (index, was) in cycle.clamped.iter().enumerate() {
        if !was {
            continue;
        }
        let mut item = Json::obj();
        item.set("channel", Json::Str(channel_name(channels, index)));
        item.set(
            "from",
            number(cycle.desired.get(index).copied().unwrap_or(f32::NAN) as f64),
        );
        item.set(
            "to",
            number(cycle.command.get(index).copied().unwrap_or(f64::NAN)),
        );
        clamped.push(item);
    }
    document.set("clamped", clamped);
    document.set("advisory", Json::Bool(cycle.advisory));
    canonical(&document)
}

fn command_message(record: &Record, cycle: &Cycle, channels: &[String]) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("tick", Json::Num(cycle.tick as f64));
    document.set(
        "values",
        values_array(channels, &cycle.command, &cycle.mask),
    );
    document.set("rhal_result", Json::Num(cycle.rhal_result as f64));
    canonical(&document)
}

fn frame_message(record: &Record, frame: &Frame) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("observation", Json::Str(hex(&frame.observation)));
    document.set("target_ns", Json::Num(frame.target_ns as f64));
    // Present-and-null rather than absent or zero. A frame with fewer than two contributing streams
    // has no spread to report, and that is a *fact* about the frame rather than a missing field —
    // the same distinction §7.2 draws between "nobody declared an envelope" and "the envelope is
    // null", arriving in a recording somebody will read months later.
    document.set(
        "worst_skew_us",
        match frame.worst_skew_us {
            Some(value) => Json::Num(value as f64),
            None => Json::Null,
        },
    );
    document.set("streams_omitted", Json::Num(frame.streams_omitted as f64));
    document.set("streams_absent", Json::Num(frame.streams_absent as f64));
    let mut streams = Json::arr();
    for stream in &frame.streams {
        let mut item = Json::obj();
        item.set("name", Json::Str(stream.name.clone()));
        item.set("age_us", Json::Num(stream.age_us as f64));
        item.set("stale", Json::Bool(stream.stale));
        streams.push(item);
    }
    document.set("streams", streams);
    canonical(&document)
}

fn event_message(record: &Record, event: &Event) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("event", Json::Str(event.code.as_str().into()));
    document.set("subject", Json::Str(event.subject.clone()));
    document.set("detail", Json::Str(event.detail.clone()));
    canonical(&document)
}

fn gap_message(record: &Record, gap: &Gap) -> String {
    let mut document = Json::obj();
    document.set("mono_ns", Json::Num(record.mono_ns as f64));
    document.set("event", Json::Str("gap".into()));
    document.set("subject", Json::Str(gap.reason.as_str().into()));
    document.set(
        "detail",
        Json::Str(format!(
            "{} records were not written, from tick {}",
            gap.dropped, gap.first_tick
        )),
    );
    document.set("dropped", Json::Num(gap.dropped as f64));
    document.set("first_tick", Json::Num(gap.first_tick as f64));
    document.set("reason", Json::Str(gap.reason.as_str().into()));
    canonical(&document)
}
