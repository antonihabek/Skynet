//! Observation assembly (§25.1).
//!
//! §25.1 calls this "one of the harder parts of the whole system", and the reason is a sentence
//! worth keeping in front of you while reading the rest of this module:
//!
//! > A policy is trained on synchronised frames: image and joint state as of one instant. A real
//! > robot has three cameras at 30 Hz with 20–60 ms of pipeline latency, joint state at 1 kHz with
//! > 1 ms, and an IMU at 200 Hz — none of them aligned, none of them stamped the same way. Feed a
//! > policy the most recent value from each and it receives **a frame that never existed**, in
//! > which the arm is where it is *now* and the image shows where it was two frames ago. Policies
//! > degrade in ways that look like bad training.
//!
//! The last clause is the expensive part. A skewed frame does not announce itself: it presents as a
//! policy that grasps slightly late, or oscillates on contact, or works on the bench and not on the
//! robot — every one of which sends somebody to retrain a model that was fine.
//!
//! # What this module does, and the one thing it does not
//!
//! It does not synchronise anything. Nothing here can make an unsynchronised robot into a
//! synchronised one, and §25.1 is explicit that the goal is narrower and achievable: **the skew is
//! measured and visible instead of being silently folded into the policy's behaviour.**
//!
//! So [`assemble`] targets an *instant* rather than collecting latest values, picks the sample
//! nearest that instant for each stream, and reports what it had to accept in order to do so.
//!
//! # Four rules, each from a line of §25.1
//!
//! 1. **One time base.** Every stamp here is `CLOCK_MONOTONIC_RAW` nanoseconds, at acquisition,
//!    never at arrival (§35.1, §47.2). This module takes no clock: the target instant is an
//!    argument, for the reason §35.2 gives for the kernel — a frame vector carries its own time, so
//!    replay and simulation need no special case.
//! 2. **Declared per-sensor latency**, so the assembler knows how far back a camera's *now*
//!    actually is. [`Stream::latency_ms`] comes from the embodiment descriptor (§52) and is
//!    subtracted from the stamp to give the instant the sample actually describes.
//! 3. **A staleness budget per stream**, from the policy's declared observation requirements.
//!    Exceeding it is a **hold**, not a silently older frame.
//! 4. **Skew and per-stream ages go into the black box**, so "the policy behaved oddly" can be
//!    checked against "the frame it was given was 80 ms skewed" months later. [`Frame::to_json`] is
//!    that record.
//!
//! # Why an undeclared latency is not a failure, and not a zero
//!
//! §7.2's rule, applied to the one place where breaking it would be invisible. A stream that has
//! not declared its latency is assembled at its raw stamp — there is nothing else to do with it —
//! and the frame says so in [`Frame::undeclared_latency`]. What it does **not** do is pretend the
//! omission cost nothing: [`Frame::skew_upper_ms`] is `None` whenever any stream's latency is
//! undeclared, because the true skew is then larger by an unknown amount and a number that claims
//! otherwise is worse than no number.
//!
//! Zero would be a claim — a sensor whose sample is available the instant the event happens — and
//! it is a claim no imaging pipeline can make.
//!
//! # Uncertainty, and where it comes from
//!
//! §47.2: where a capture backend genuinely cannot supply a capture timestamp, "the pipeline
//! records an *estimated* latency and the frame carries a larger uncertainty, which flows into the
//! skew the assembler already reports".
//!
//! [`Stream::uncertainty_ms`] is that figure, and it is deliberately **not** a descriptor field.
//! The descriptor declares what the sensor's latency is; whether a given capture path stamped at
//! capture or estimated is a property of the *backend*, which may differ between two robots
//! carrying the same camera. The vendor declares the latency; the pipeline declares how well it
//! knows it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::json::number_text;
// One rounding decision for the crate; see the note on the function.
use crate::round_half_away;

/// Nanoseconds in a millisecond. Stamps are `CLOCK_MONOTONIC_RAW` nanoseconds (§24, §35.1);
/// everything a human reads is milliseconds.
const NS_PER_MS: f64 = 1_000_000.0;

/// One sample from one stream.
///
/// Deliberately tiny: this contract decides *which* sample belongs in the frame, never what is in
/// it. The payload — an image, a joint vector, an IMU reading — never enters the contracts crate,
/// which is what lets the same function run on the Hub over a recorded frame index and on the robot
/// over a live capture ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// `CLOCK_MONOTONIC_RAW` at **acquisition**, never at return or arrival (§24, §47.2).
    pub stamp_ns: u64,
    /// The capture sequence number, carried through so a recorded frame can be tied back to the
    /// buffer it came from.
    pub seq: u64,
}

/// One stream the policy asked for, and what is available on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// The sensor's name on the embodiment descriptor.
    pub name: String,
    /// Its modality, carried so a hold can say *"the depth stream is stale"* rather than
    /// *"stream 3 is stale"*.
    pub modality: String,
    /// The declared per-sensor latency (§52), in milliseconds.
    ///
    /// How far behind the world this stream's timestamps are: the event a sample describes happened
    /// this long before the stamp the sample carries. `None` means nobody declared it — see the
    /// module documentation for why that is neither a failure nor a zero.
    pub latency_ms: Option<f64>,
    /// How wrong [`Self::latency_ms`] could be, in milliseconds (§47.2).
    ///
    /// Supplied by the capture pipeline rather than by the descriptor: a backend that stamps at
    /// capture knows the figure precisely, and one that estimates knows it badly. `None` on a
    /// stream that *has* declared a latency means the declaration is exact; on one that has not, it
    /// is redundant, because an undeclared latency is already unbounded.
    pub uncertainty_ms: Option<f64>,
    /// How old a sample may be before this stream is unusable, in milliseconds.
    ///
    /// From the policy's declared observation requirements, not from the sensor: the same camera is
    /// fine at 200 ms for a mapping policy and useless at 40 ms for a contact-rich manipulation
    /// one. `None` means the policy declared no budget for this stream, which is not a licence to
    /// use any sample at all — it means nothing here can say whether the one chosen is good enough,
    /// and the frame reports it.
    pub budget_ms: Option<f64>,
    /// Every sample available for this stream, in any order.
    ///
    /// Order does not matter to [`assemble`] — it minimises a distance — but it must not *change*
    /// the answer either, which is what the tie-break in [`Chosen`] is for.
    pub samples: Vec<Sample>,
}

/// The sample [`assemble`] picked for a stream, and what accepting it cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chosen {
    /// The stamp the sample carried.
    pub stamp_ns: u64,
    /// Its capture sequence.
    pub seq: u64,
    /// The instant the sample actually describes: its stamp, less the declared latency.
    ///
    /// Signed, because a stream whose declared latency exceeds its stamp is a misconfiguration
    /// rather than a reason to saturate at zero and hide it.
    pub effective_ns: i128,
    /// How far behind the target instant that is, in milliseconds.
    ///
    /// **Signed.** Negative means the chosen sample describes an instant *after* the one the frame
    /// claims to be, which cannot happen on a live robot and happens routinely in replay. Clamping
    /// it at zero would erase the only evidence of a clock that ran backwards.
    pub age_ms: f64,
    /// Whether [`Self::age_ms`] exceeds the stream's budget.
    pub over_budget: bool,
    /// How many samples were available to choose from.
    ///
    /// One is not an error and is worth seeing: a stream that offers a single sample every cycle is
    /// a stream with no slack, and the frame it produces is the frame it produced, not the best of
    /// several.
    pub considered: usize,
}

/// Why a frame cannot be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    /// The stream that caused it.
    pub stream: String,
    /// `no_sample` or `stale`. Two codes because they send a person to two different places: an
    /// empty stream is a capture path that is not running, and a stale one is a capture path that
    /// is running too slowly or a budget that is too tight.
    pub reason: &'static str,
    /// The sentence, naming the numbers.
    pub detail: String,
}

/// What one stream contributed to the frame.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamFrame {
    /// The sensor's name.
    pub name: String,
    /// Its modality.
    pub modality: String,
    /// The declared latency, carried into the record so a replay does not have to go and find the
    /// descriptor that was current at the time.
    pub latency_ms: Option<f64>,
    /// Its declared uncertainty.
    pub uncertainty_ms: Option<f64>,
    /// The budget it was held to.
    pub budget_ms: Option<f64>,
    /// The sample chosen, or `None` when the stream offered none.
    pub chosen: Option<Chosen>,
}

/// One assembled observation frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The instant this frame claims to describe.
    pub target_ns: u64,
    /// Every stream, in the order they were given — which is the descriptor's order, and the only
    /// order in which a reader can compare two frames row by row.
    pub streams: Vec<StreamFrame>,
    /// The spread between the earliest and latest instants actually in the frame, in milliseconds.
    ///
    /// This is the number §25.1 exists to make visible. `None` when fewer than two streams
    /// contributed a sample, because the spread of one instant is not zero — it is undefined, and
    /// zero is the answer that would let a half-empty frame look perfectly synchronised.
    pub skew_ms: Option<f64>,
    /// The spread once each stream's latency uncertainty is taken at its worst, in milliseconds.
    ///
    /// `None` when any stream's latency is undeclared: the widening is then unbounded, and §47.2's
    /// "larger uncertainty" has no largest.
    pub skew_upper_ms: Option<f64>,
    /// Streams that did not declare a latency, in frame order.
    pub undeclared_latency: Vec<String>,
    /// Streams the policy set no budget for, in frame order.
    ///
    /// Separate from a hold, and separate from an undeclared latency: this one is a gap in the
    /// *package's* declaration rather than the body's, and the remedy is a different person.
    pub undeclared_budget: Vec<String>,
    /// Every reason this frame may not be delivered, in frame order.
    pub holds: Vec<Hold>,
}

/// One assembled frame, projected onto what §26's `/sk/frame` record can carry.
///
/// §25.1's last bullet: *"The frame's skew and per-stream ages go into the black box, so 'the policy
/// behaved oddly' can be checked against 'the frame it was given was 80 ms skewed' months later."*
/// The record exists — channel 5, `sk_frame`, written in the sixth session — and this is the
/// projection onto it.
///
/// It is a *projection* rather than a rename, and the two places it loses something are stated here
/// rather than discovered by whoever reads a recording:
///
/// * **`age_us` cannot be negative.** The record's field is unsigned, and a frame whose chosen
///   sample is *after* the target instant saturates at zero. That cannot arise on a live robot —
///   every target a runtime picks is at or before now, and `CLOCK_MONOTONIC_RAW` does not step —
///   which is why the saturation is acceptable rather than merely convenient. [`Frame::streams`]
///   keeps the signed figure for anything that is not the recorder.
/// * **`worst_skew_us` is `None` when the skew is not measurable.** [`Frame::skew_ms`] is `None` when
///   fewer than two streams contributed, because the spread of one instant is undefined. The record
///   format carries it — `skynet_blackbox::record::SKEW_UNMEASURABLE`, added in format version 2 for
///   this — because writing zero would make a frame with one working stream and three dead capture
///   paths read, months later, as perfectly synchronised.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The instant the assembler targeted.
    pub target_ns: u64,
    /// The worst skew in the frame, in microseconds, or `None` when it is not measurable.
    pub worst_skew_us: Option<u32>,
    /// Streams that were declared and produced no sample.
    ///
    /// Distinct from the record's `streams_omitted`, which is *"streams the record could not
    /// fit"* — a limit of the format. This is a limit of the robot, and an investigation reading
    /// one as the other would be looking for a bigger record instead of a dead camera. Version 1 of
    /// the record had no field for it at all, which is one of the two reasons there is a version 2.
    pub streams_absent: u16,
    /// One row per contributing stream, in frame order.
    pub streams: Vec<RecordStream>,
}

/// One contributing stream, as the record carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordStream {
    /// The stream's name. The record truncates it to sixteen bytes; that is the record's business.
    pub name: String,
    /// How far back its sample was, in microseconds, saturating at zero.
    pub age_us: u32,
    /// Whether that broke the policy's budget.
    pub stale: bool,
}

impl Frame {
    /// Project this frame onto what the black box records (§25.1, §26).
    pub fn to_record(&self) -> Record {
        let mut streams = Vec::new();
        let mut absent = 0u16;
        for stream in &self.streams {
            match stream.chosen {
                None => absent = absent.saturating_add(1),
                Some(chosen) => streams.push(RecordStream {
                    name: stream.name.clone(),
                    age_us: micros(chosen.age_ms),
                    stale: chosen.over_budget,
                }),
            }
        }
        Record {
            target_ns: self.target_ns,
            worst_skew_us: self.skew_ms.map(micros),
            streams_absent: absent,
            streams,
        }
    }
}

/// Milliseconds as whole microseconds, saturating at zero and at the top of a `u32`.
///
/// The saturation at the top is not a real case — it is seventy-one minutes of skew — and it is
/// here because the alternative in release builds is a wrap, and a frame recorded as four
/// microseconds skewed when it was four thousand seconds skewed is worse than one recorded at the
/// ceiling.
fn micros(ms: f64) -> u32 {
    let micros = round_half_away(ms * 1000.0);
    if micros <= 0.0 {
        0
    } else if micros >= u32::MAX as f64 {
        u32::MAX
    } else {
        micros as u32
    }
}

impl Frame {
    /// Whether this frame may be given to a policy.
    ///
    /// §25.1: exceeding a staleness budget "is a `hold`, not a silently older frame". So is an
    /// empty stream. Everything else — an undeclared latency, an undeclared budget, an unbounded
    /// skew — is *reported* and does not block, because none of them is evidence that the frame is
    /// wrong. They are evidence that nobody can say how wrong it is, which is a different sentence
    /// and belongs on a console rather than in a control decision.
    pub fn deliverable(&self) -> bool {
        self.holds.is_empty()
    }

    /// `ok` or `hold`, for the record and the console.
    pub fn verdict(&self) -> &'static str {
        if self.deliverable() {
            "ok"
        } else {
            "hold"
        }
    }

    /// The frame as the black box records it (§25.1's last bullet).
    ///
    /// Payloads are absent by construction — this is the index of a frame, not the frame — which is
    /// what makes it affordable to record every cycle. An incident investigation months later reads
    /// this and can say which sample went in and how far apart they were, which is exactly the
    /// question "the policy behaved oddly" turns into.
    pub fn to_json(&self) -> Json {
        let mut streams = Json::arr();
        for stream in &self.streams {
            let mut row = Json::obj();
            row.set("name", Json::Str(stream.name.clone()));
            row.set("modality", Json::Str(stream.modality.clone()));
            row.set("latency_ms", num_or_null(stream.latency_ms));
            row.set("uncertainty_ms", num_or_null(stream.uncertainty_ms));
            row.set("budget_ms", num_or_null(stream.budget_ms));
            match &stream.chosen {
                Some(chosen) => {
                    row.set("age_ms", Json::Num(chosen.age_ms));
                    row.set("considered", Json::Num(chosen.considered as f64));
                    row.set("over_budget", Json::Bool(chosen.over_budget));
                    row.set("seq", Json::Num(chosen.seq as f64));
                    row.set("stamp_ns", Json::Num(chosen.stamp_ns as f64));
                }
                None => {
                    // Five nulls rather than five absent keys. A recorded frame is read by a
                    // console that lays the streams out in a table, and a row with no columns is a
                    // row that renders as blank rather than as *"this stream had nothing"*.
                    row.set("age_ms", Json::Null);
                    row.set("considered", Json::Num(0.0));
                    row.set("over_budget", Json::Bool(false));
                    row.set("seq", Json::Null);
                    row.set("stamp_ns", Json::Null);
                }
            }
            streams.push(row);
        }

        let mut holds = Json::arr();
        for hold in &self.holds {
            let mut row = Json::obj();
            row.set("stream", Json::Str(hold.stream.clone()));
            row.set("reason", Json::Str(hold.reason.to_string()));
            row.set("detail", Json::Str(hold.detail.clone()));
            holds.push(row);
        }

        let mut out = Json::obj();
        out.set("target_ns", Json::Num(self.target_ns as f64));
        out.set("streams", streams);
        out.set("skew_ms", num_or_null(self.skew_ms));
        out.set("skew_upper_ms", num_or_null(self.skew_upper_ms));
        out.set("undeclared_latency", str_list(&self.undeclared_latency));
        out.set("undeclared_budget", str_list(&self.undeclared_budget));
        out.set("holds", holds);
        out.set("verdict", Json::Str(self.verdict().to_string()));
        out.set("deliverable", Json::Bool(self.deliverable()));

        // The projection onto §26's record, inside the frame rather than beside it. It is derived
        // from everything above, and pinning it in a second corpus would let an edition agree about
        // a frame and disagree about the only version of it that survives the day.
        let record = self.to_record();
        let mut record_streams = Json::arr();
        for stream in &record.streams {
            let mut row = Json::obj();
            row.set("name", Json::Str(stream.name.clone()));
            row.set("age_us", Json::Num(stream.age_us as f64));
            row.set("stale", Json::Bool(stream.stale));
            record_streams.push(row);
        }
        let mut recorded = Json::obj();
        recorded.set("target_ns", Json::Num(record.target_ns as f64));
        recorded.set(
            "worst_skew_us",
            match record.worst_skew_us {
                Some(value) => Json::Num(value as f64),
                None => Json::Null,
            },
        );
        recorded.set("streams_absent", Json::Num(record.streams_absent as f64));
        recorded.set("streams", record_streams);
        out.set("record", recorded);
        out
    }
}

fn num_or_null(value: Option<f64>) -> Json {
    match value {
        Some(number) => Json::Num(number),
        None => Json::Null,
    }
}

fn str_list(values: &[String]) -> Json {
    Json::Arr(values.iter().map(|s| Json::Str(s.clone())).collect())
}

/// The instant a sample describes: its stamp, less the declared latency.
///
/// Exact integer arithmetic on the nanoseconds, and `i128` rather than `i64` so a stamp near the
/// top of a `u64` cannot overflow the subtraction. The latency arrives in milliseconds because
/// that is what a vendor writes on a datasheet; it is converted here, once, and rounded to the
/// nearest nanosecond so that two editions performing the same conversion agree on the bit.
fn effective_ns(stamp_ns: u64, latency_ms: Option<f64>) -> i128 {
    let latency_ns = match latency_ms {
        Some(ms) => round_half_away(ms * NS_PER_MS) as i128,
        None => 0,
    };
    stamp_ns as i128 - latency_ns
}

/// Nanoseconds as milliseconds.
///
/// One division, on values that are exact integers, so every edition performing the same division
/// gets the same bits — IEEE-754 division is correctly rounded everywhere, which is the property
/// §31.2 leans on for the kernel's `sin` and `cos` and which applies here for the same reason.
fn ms(nanos: i128) -> f64 {
    nanos as f64 / NS_PER_MS
}

/// Assemble one observation frame for the given instant (§25.1).
///
/// For each stream: correct every sample for the declared latency, pick the one nearest the target,
/// and measure how far behind the target it is. Then measure the spread across the streams, which
/// is the frame's own internal inconsistency — the number that separates "the policy behaved
/// oddly" from "the frame it was given was 80 ms skewed".
///
/// # Nearest, and what happens on a tie
///
/// Nearest is by absolute distance, so a sample *after* the target can be chosen when it is
/// genuinely closer. On a live robot that cannot arise — the target is at or before now — and in
/// replay it is both possible and correct.
///
/// A tie takes the **earlier** sample. Two samples equidistant either side of the target are
/// equally good by the distance rule and are not equally good in any other sense: the earlier one
/// describes something that had already happened at the instant the frame claims to describe, and
/// the later one does not. Choosing the earlier is choosing the frame that could have existed.
///
/// The tie-break also has to exist for a duller reason: without it, two editions iterating the
/// samples in different orders would each pick a different sample and the corpus would diverge on
/// an input where nothing is wrong.
pub fn assemble(target_ns: u64, streams: &[Stream]) -> Frame {
    let target = target_ns as i128;
    let mut frames: Vec<StreamFrame> = Vec::new();
    let mut holds: Vec<Hold> = Vec::new();
    let mut undeclared_latency: Vec<String> = Vec::new();
    let mut undeclared_budget: Vec<String> = Vec::new();

    for stream in streams {
        if stream.latency_ms.is_none() {
            undeclared_latency.push(stream.name.clone());
        }
        if stream.budget_ms.is_none() {
            undeclared_budget.push(stream.name.clone());
        }

        let mut best: Option<(i128, Sample)> = None;
        for sample in &stream.samples {
            let effective = effective_ns(sample.stamp_ns, stream.latency_ms);
            let distance = (effective - target).abs();
            let better = match best {
                None => true,
                Some((best_effective, _)) => {
                    let best_distance = (best_effective - target).abs();
                    // Strictly nearer wins; on a tie the earlier instant wins. Never "the one seen
                    // first", which would make the answer depend on the order the capture ring
                    // happened to be walked in.
                    distance < best_distance
                        || (distance == best_distance && effective < best_effective)
                }
            };
            if better {
                best = Some((effective, *sample));
            }
        }

        let chosen = match best {
            None => {
                holds.push(Hold {
                    stream: stream.name.clone(),
                    reason: "no_sample",
                    detail: format!(
                        "{} ({}) offered no sample for this instant, so there is no frame to \
                         assemble. A stream the policy declared and the pipeline is not producing \
                         is a capture path that is not running.",
                        stream.name, stream.modality
                    ),
                });
                None
            }
            Some((effective, sample)) => {
                let age = ms(target - effective);
                let over_budget = match stream.budget_ms {
                    // One-sided, and deliberately so. The budget bounds how far *behind* the target
                    // a sample may be; a sample from ahead of it is a clock fault rather than a
                    // stale one, it is visible in the signed age, and reporting it as staleness
                    // would send somebody to look at a capture pipeline that is working.
                    Some(budget) => age > budget,
                    None => false,
                };
                if over_budget {
                    let budget = stream.budget_ms.unwrap_or(0.0);
                    holds.push(Hold {
                        stream: stream.name.clone(),
                        reason: "stale",
                        detail: format!(
                            "{} ({}) is {} ms behind the frame's instant and the policy's budget \
                             for it is {} ms. §25.1: exceeding it is a hold, not a silently older \
                             frame.",
                            stream.name,
                            stream.modality,
                            number_text(age),
                            number_text(budget)
                        ),
                    });
                }
                Some(Chosen {
                    stamp_ns: sample.stamp_ns,
                    seq: sample.seq,
                    effective_ns: effective,
                    age_ms: age,
                    over_budget,
                    considered: stream.samples.len(),
                })
            }
        };

        frames.push(StreamFrame {
            name: stream.name.clone(),
            modality: stream.modality.clone(),
            latency_ms: stream.latency_ms,
            uncertainty_ms: stream.uncertainty_ms,
            budget_ms: stream.budget_ms,
            chosen,
        });
    }

    let (skew_ms, skew_upper_ms) = skew(&frames, &undeclared_latency);

    Frame {
        target_ns,
        streams: frames,
        skew_ms,
        skew_upper_ms,
        undeclared_latency,
        undeclared_budget,
        holds,
    }
}

/// The frame's internal spread, measured and at its worst.
///
/// Measured: the difference between the latest and earliest instants actually in the frame.
///
/// At its worst: each stream's instant could be anywhere within its declared uncertainty, so the
/// widest the frame could be is the measured spread plus the two extreme uncertainties. Returns
/// `None` for the upper bound whenever any stream's latency is undeclared, because the widening is
/// then unbounded — §47.2 asks for "a larger uncertainty", and an unknown offset has no larger.
fn skew(frames: &[StreamFrame], undeclared_latency: &[String]) -> (Option<f64>, Option<f64>) {
    let mut lowest: Option<(i128, f64)> = None;
    let mut highest: Option<(i128, f64)> = None;
    let mut contributors = 0usize;

    for frame in frames {
        let Some(chosen) = frame.chosen else { continue };
        contributors += 1;
        let uncertainty = frame.uncertainty_ms.unwrap_or(0.0);
        if lowest.is_none_or(|(instant, _)| chosen.effective_ns < instant) {
            lowest = Some((chosen.effective_ns, uncertainty));
        }
        if highest.is_none_or(|(instant, _)| chosen.effective_ns > instant) {
            highest = Some((chosen.effective_ns, uncertainty));
        }
    }

    // One instant has no spread, and reporting zero would let a frame with a single working stream
    // look perfectly synchronised — which is the presentation §25.1 is written against.
    if contributors < 2 {
        return (None, None);
    }

    let (low, low_uncertainty) = lowest.unwrap_or((0, 0.0));
    let (high, high_uncertainty) = highest.unwrap_or((0, 0.0));
    let measured = ms(high - low);

    if !undeclared_latency.is_empty() {
        return (Some(measured), None);
    }

    (
        Some(measured),
        Some(measured + low_uncertainty + high_uncertainty),
    )
}

/// The most recent instant at which every stream has a sample (§25.1's "targets an instant").
///
/// A live control loop has to choose a target, and choosing badly defeats the whole section: aim at
/// *now* and the slowest camera is always the stalest stream in every frame, so every frame is
/// skewed by that camera's latency and the assembler reports a problem the loop created. Aim here
/// instead and the frame is the most recent one that every stream can actually answer for.
///
/// Returns `None` when any stream has no sample at or before the given instant, which is the same
/// fact [`assemble`] reports as a `no_sample` hold and is worth having before building a frame
/// nobody can use.
///
/// This is offered rather than imposed. A runtime may target an older instant deliberately — a
/// policy that tolerates latency and hates skew is better served by a target every stream is
/// comfortably behind — and nothing here prevents that. What the function exists to prevent is two
/// editions inventing two different defaults and disagreeing about frames neither of them
/// assembled wrongly.
pub fn latest_common_instant(now_ns: u64, streams: &[Stream]) -> Option<u64> {
    let now = now_ns as i128;
    let mut earliest: Option<i128> = None;

    for stream in streams {
        let mut latest: Option<i128> = None;
        for sample in &stream.samples {
            let effective = effective_ns(sample.stamp_ns, stream.latency_ms);
            if effective > now {
                continue;
            }
            if latest.is_none_or(|current| effective > current) {
                latest = Some(effective);
            }
        }
        // A stream with nothing at or before `now` cannot contribute to any frame at or before
        // `now`, so there is no common instant at all — not a later one, and not one that quietly
        // drops the stream.
        let latest = latest?;
        if earliest.is_none_or(|current| latest < current) {
            earliest = Some(latest);
        }
    }

    // No streams at all: every instant is common to all of them, and the honest answer to "the
    // latest instant every stream can answer for" when there are no streams is the one asked about.
    let earliest = earliest.unwrap_or(now);
    if earliest < 0 {
        // A declared latency larger than the stamp itself. The correction is still right and the
        // instant it names is before the monotonic clock started, which no `u64` can express; the
        // caller gets `None` rather than a wrapped value.
        return None;
    }
    Some(earliest as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn sample(stamp_ns: u64, seq: u64) -> Sample {
        Sample { stamp_ns, seq }
    }

    fn stream(
        name: &str,
        latency_ms: Option<f64>,
        budget_ms: Option<f64>,
        samples: Vec<Sample>,
    ) -> Stream {
        Stream {
            name: name.to_string(),
            modality: "rgb".to_string(),
            latency_ms,
            uncertainty_ms: None,
            budget_ms,
            samples,
        }
    }

    #[test]
    fn the_declared_latency_moves_the_sample_back_in_time() {
        // A camera stamped at 1.000 s that is 28 ms behind the world describes 0.972 s.
        let frame = assemble(
            1_000_000_000,
            &[stream(
                "cam",
                Some(28.0),
                Some(60.0),
                vec![sample(1_000_000_000, 7)],
            )],
        );
        let chosen = frame.streams[0].chosen.expect("a sample");
        assert_eq!(chosen.effective_ns, 972_000_000);
        assert_eq!(chosen.age_ms, 28.0);
        assert!(frame.deliverable());
    }

    #[test]
    fn nearest_wins_and_a_tie_takes_the_earlier_sample() {
        // 40 ms either side of the target, latency zero. The earlier one is chosen, and choosing it
        // is not an accident of iteration order — reversing the samples must not change it.
        let forward = assemble(
            1_000_000_000,
            &[stream(
                "j",
                Some(0.0),
                None,
                vec![sample(960_000_000, 1), sample(1_040_000_000, 2)],
            )],
        );
        let backward = assemble(
            1_000_000_000,
            &[stream(
                "j",
                Some(0.0),
                None,
                vec![sample(1_040_000_000, 2), sample(960_000_000, 1)],
            )],
        );
        assert_eq!(forward.streams[0].chosen.unwrap().seq, 1);
        assert_eq!(backward.streams[0].chosen.unwrap().seq, 1);
    }

    #[test]
    fn a_strictly_nearer_later_sample_is_chosen_and_its_age_is_negative() {
        let frame = assemble(
            1_000_000_000,
            &[stream(
                "j",
                Some(0.0),
                None,
                vec![sample(900_000_000, 1), sample(1_010_000_000, 2)],
            )],
        );
        let chosen = frame.streams[0].chosen.unwrap();
        assert_eq!(chosen.seq, 2);
        assert_eq!(chosen.age_ms, -10.0);
        // Ahead of the target is not stale. It is a clock fault, and it has its own evidence.
        assert!(!chosen.over_budget);
    }

    #[test]
    fn exceeding_the_budget_is_a_hold() {
        let frame = assemble(
            1_000_000_000,
            &[stream(
                "cam",
                Some(0.0),
                Some(40.0),
                vec![sample(900_000_000, 1)],
            )],
        );
        assert!(!frame.deliverable());
        assert_eq!(frame.verdict(), "hold");
        assert_eq!(frame.holds[0].reason, "stale");
        assert!(frame.holds[0].detail.contains("100 ms"));
    }

    #[test]
    fn an_empty_stream_holds_and_says_which() {
        let frame = assemble(
            1_000_000_000,
            &[stream("depth", Some(5.0), Some(40.0), vec![])],
        );
        assert_eq!(frame.holds.len(), 1);
        assert_eq!(frame.holds[0].reason, "no_sample");
        assert_eq!(frame.holds[0].stream, "depth");
        assert!(frame.streams[0].chosen.is_none());
    }

    #[test]
    fn an_undeclared_latency_is_named_and_does_not_hold() {
        let frame = assemble(
            1_000_000_000,
            &[
                stream("cam", None, Some(200.0), vec![sample(980_000_000, 1)]),
                stream(
                    "joints",
                    Some(1.0),
                    Some(10.0),
                    vec![sample(999_000_000, 2)],
                ),
            ],
        );
        assert!(frame.deliverable());
        assert_eq!(frame.undeclared_latency, vec!["cam".to_string()]);
        // Measured skew is still reported; the *bound* on it is not, because the camera's true
        // instant could be anywhere.
        assert_eq!(frame.skew_ms, Some(18.0));
        assert_eq!(frame.skew_upper_ms, None);
    }

    #[test]
    fn uncertainty_widens_the_bound_without_touching_the_measurement() {
        let mut cam = stream(
            "cam",
            Some(30.0),
            Some(200.0),
            vec![sample(1_000_000_000, 1)],
        );
        cam.uncertainty_ms = Some(4.0);
        let mut joints = stream(
            "joints",
            Some(1.0),
            Some(10.0),
            vec![sample(1_000_000_000, 2)],
        );
        joints.uncertainty_ms = Some(0.5);
        let frame = assemble(1_000_000_000, &[cam, joints]);
        assert_eq!(frame.skew_ms, Some(29.0));
        assert_eq!(frame.skew_upper_ms, Some(33.5));
    }

    #[test]
    fn one_contributing_stream_has_no_skew_rather_than_zero_skew() {
        let frame = assemble(
            1_000_000_000,
            &[
                stream("cam", Some(0.0), None, vec![sample(1_000_000_000, 1)]),
                stream("depth", Some(0.0), None, vec![]),
            ],
        );
        assert_eq!(frame.skew_ms, None);
        assert_eq!(frame.skew_upper_ms, None);
    }

    #[test]
    fn the_latest_common_instant_is_the_slowest_streams_latest_sample() {
        let streams = [
            stream("cam", Some(30.0), None, vec![sample(1_000_000_000, 1)]),
            stream("joints", Some(1.0), None, vec![sample(1_020_000_000, 2)]),
        ];
        // The camera describes 0.970 s and the joints describe 1.019 s; the frame everybody can
        // answer for is at 0.970 s.
        assert_eq!(
            latest_common_instant(1_100_000_000, &streams),
            Some(970_000_000)
        );
    }

    #[test]
    fn a_stream_with_nothing_yet_has_no_common_instant() {
        let streams = [
            stream("cam", Some(0.0), None, vec![sample(1_000_000_000, 1)]),
            stream("depth", Some(0.0), None, vec![sample(2_000_000_000, 2)]),
        ];
        // The depth sample is in the future relative to `now`, so there is no instant at or before
        // `now` that every stream can answer for.
        assert_eq!(latest_common_instant(1_500_000_000, &streams), None);
    }

    #[test]
    fn an_absent_stream_is_counted_rather_than_dropped_from_the_record() {
        // The record carries a row per *contributing* stream, so a dead capture path leaves no row
        // at all. Counting it is what stops a recording of a robot with one working camera and two
        // dead ones reading, months later, as a recording of a robot with one camera.
        let frame = assemble(
            1_000_000_000,
            &[
                stream("cam", Some(0.0), Some(200.0), vec![sample(980_000_000, 1)]),
                stream("depth", Some(0.0), Some(200.0), vec![]),
                stream("lidar", Some(0.0), Some(200.0), vec![]),
            ],
        );
        let record = frame.to_record();
        assert_eq!(record.streams.len(), 1);
        assert_eq!(record.streams_absent, 2);
        assert_eq!(record.streams[0].age_us, 20_000);
        // One contributor, so there is no spread — and the record leaves it unset rather than
        // writing the zero that would read as perfect synchronisation.
        assert_eq!(record.worst_skew_us, None);
    }

    #[test]
    fn a_negative_age_saturates_in_the_record_and_not_on_the_frame() {
        // The one thing the projection loses, and both halves are asserted so that losing it is a
        // decision somebody made rather than a field somebody forgot.
        let frame = assemble(
            1_000_000_000,
            &[
                stream("j", Some(0.0), None, vec![sample(1_004_000_000, 1)]),
                stream("k", Some(0.0), None, vec![sample(990_000_000, 2)]),
            ],
        );
        assert_eq!(frame.streams[0].chosen.unwrap().age_ms, -4.0);
        let record = frame.to_record();
        assert_eq!(record.streams[0].age_us, 0);
        assert_eq!(record.streams[1].age_us, 10_000);
        assert_eq!(record.worst_skew_us, Some(14_000));
    }

    #[test]
    fn a_stale_stream_is_marked_stale_in_the_record() {
        let frame = assemble(
            1_000_000_000,
            &[
                stream("cam", Some(0.0), Some(40.0), vec![sample(900_000_000, 1)]),
                stream("j", Some(0.0), Some(40.0), vec![sample(999_000_000, 2)]),
            ],
        );
        let record = frame.to_record();
        assert!(record.streams[0].stale);
        assert!(!record.streams[1].stale);
    }

    #[test]
    fn a_frame_records_itself_in_canonical_form() {
        let frame = assemble(
            1_000_000_000,
            &[stream(
                "cam",
                Some(28.0),
                Some(60.0),
                vec![sample(1_000_000_000, 7)],
            )],
        );
        let text = frame.to_json().to_canonical().expect("canonical");
        assert!(text.contains("\"verdict\":\"ok\""));
        assert!(text.contains("\"age_ms\":28"));
        assert!(text.contains("\"skew_ms\":null"));
    }
}
