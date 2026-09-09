//! The four messages back into a cycle (§13, §26, §83.5).
//!
//! [`read`](crate::read) answers *what is in this file* — provenance, whether it is whole, and the
//! events. [`replay`](crate::replay) answers *would this window still produce these verdicts*, and
//! it was written to take records and name tables rather than a [`Ring`](crate::ring::Ring)
//! precisely so that a recording read off a file could reach it. Nothing joined them, and this is
//! the join.
//!
//! A cycle is sealed as **four messages** — `/sk/state`, `/sk/desired`, `/sk/verdict`,
//! `/sk/command` — and reassembly is the inverse of that split. It is the harder half of reading a
//! recording and it is the half that makes one evidence rather than a document: until it existed,
//! every question of the form *would this robot do that again* had to be asked of the robot, which
//! is the machine that is broken when somebody is asking.
//!
//! # What the file cannot say, and why each is named rather than filled
//!
//! Three of [`replay`](crate::replay)'s inputs reached the ring and did not reach the file until
//! 2026-09-09, and two of them are inputs the Safety Kernel *evaluates*:
//!
//! * **`capability`** — §7.2's mode gate treats *no capability* and *the runtime did not say*
//!   differently, so a replay that assumed the second for every cycle would recompute a different
//!   verdict and present the difference as the kernel disagreeing with itself;
//! * **`nearest_person_m`** — the proximity guard's whole input. Absent, every cycle replays as a
//!   robot with nobody near it;
//! * **`read_result`** — not an input to the verdict, but §26.4's fact about whether the body
//!   fields in a cycle are measurements at all.
//!
//! All three are in the schema now. A recording sealed **before** they were is not thereby
//! unreadable, and it must not be silently wrong either: the missing keys become exactly the values
//! the ring uses for *not known* — [`NO_CAPABILITY`], `NaN`, and [`ReadOutcome::Unrecorded`] — and
//! [`Assembled`] **counts** the cycles that lacked each, so a caller can say *this window predates
//! the field* instead of publishing a verdict comparison that rests on a guess.
//!
//! # The join is by sequence, and then checked rather than trusted
//!
//! MCAP numbers messages per channel, so the *n*th `/sk/state` and the *n*th `/sk/desired` are the
//! two halves of one cycle. Reading that as *the same position in each list* is the obvious join
//! and the wrong one: a file truncated between two of a cycle's four messages leaves the channels
//! one apart for the whole rest of the window, and every cycle after the cut is then assembled out
//! of two different cycles — each field plausible, the whole thing wrong, and nothing anywhere
//! saying so.
//!
//! So the four are matched on the channel **sequence** itself, which turns that cut into one
//! reported gap and leaves every other cycle in the file intact. They are then **required to agree
//! about `mono_ns`**, which every message carries for §35.1's reasons and which is the instant the
//! record was produced. A cycle whose four messages disagree about when they happened is not a
//! cycle, and it is reported ([`Incomplete`]) rather than assembled.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;
use skynet_kernel::{FaultResponse, Guard};

use crate::read::{Message, Recording};
use crate::record::{Body, Cycle, ReadOutcome, Record, NO_CAPABILITY};
use crate::schema::id;

/// A mode this edition has no name for.
///
/// The seal writes `mode` as a name and spells everything outside §24's three as `"unknown"`, so
/// the original byte does not survive a round trip. Reassembly must put *something* in
/// [`Cycle::mode`], and it must not be 0, 1 or 2 — a recording that came back claiming a torque
/// command was a position command would be worse than one that says it does not know.
pub const MODE_UNKNOWN: u8 = u8::MAX;

/// One cycle that could not be assembled, and what was wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incomplete {
    /// The channel sequence the four messages were joined on — the cycle's ordinal in this file.
    pub cycle: u32,
    /// What was missing or disagreed.
    pub reason: String,
}

/// How many cycles in a file did not carry a field that [`replay`](crate::replay) needs.
///
/// Zero is *the file carries it*. A count equal to the number of cycles is *this recording was
/// sealed before the field existed*, which is a fact about the file's age. Anything in between is
/// neither, and is worth saying out loud: one writer does not stop writing a field halfway through
/// a window, so a partial count means a file that has been edited or a writer with a bug.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Absent {
    /// Cycles whose `/sk/desired` carried no `capability` key.
    pub capability: usize,
    /// Cycles whose `/sk/state` carried no `nearest_person_m` key.
    pub nearest_person_m: usize,
    /// Cycles whose `/sk/state` carried no `read_result` key.
    pub read_result: usize,
}

impl Absent {
    /// Whether every field a replay evaluates was recorded for every cycle.
    ///
    /// `read_result` is deliberately **not** in this test. It changes what a reader is told about a
    /// cycle's body fields (§26.4) and no verdict is computed from it, so a window missing it
    /// replays exactly as it ran. The other two are evaluated, and a replay of a window missing
    /// either is a replay of a robot that is not quite the one that produced it.
    pub fn verdict_inputs_all_recorded(self) -> bool {
        self.capability == 0 && self.nearest_person_m == 0
    }
}

/// Why a recording could not be assembled at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssembleError {
    /// A message's payload was not a JSON object.
    NotJson {
        /// The channel it was on. See [`crate::schema::id`].
        channel: u16,
        /// The channel sequence of the message.
        sequence: u32,
    },
    /// Two cycles disagreed about the action channels, which a fixed ring geometry forbids.
    ChannelsChanged {
        /// The cycle whose channel list differed from the first one's.
        cycle: u32,
    },
}

impl AssembleError {
    /// One sentence, for an operator rather than a log.
    pub fn detail(&self) -> String {
        match self {
            AssembleError::NotJson { channel, sequence } => {
                let mut text = String::from("message ");
                text.push_str(&sequence.to_string());
                text.push_str(" on channel ");
                text.push_str(&channel.to_string());
                text.push_str(" is not a JSON object. Every channel this platform writes is.");
                text
            }
            AssembleError::ChannelsChanged { cycle } => {
                let mut text = String::from("cycle ");
                text.push_str(&cycle.to_string());
                text.push_str(
                    " names different action channels from the first one. A ring's geometry is \
                     fixed when it is opened, so this file did not come from one recorder.",
                );
                text
            }
        }
    }
}

/// A recording, put back into the shape [`replay`](crate::replay) takes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Assembled {
    /// The cycles, in file order. Only cycles: frames, events and gaps are read by
    /// [`read`](crate::read) and are not what a replay evaluates.
    pub records: Vec<Record>,
    /// The action channels, in index order, rebuilt from the values arrays.
    pub channels: Vec<String>,
    /// The capability tokens, in first-seen order.
    ///
    /// **Not the ring's order.** [`Cycle::capability`] indexes this table, and reassembly builds
    /// both together, so a replay resolves every token to the name the file recorded. What is lost
    /// is only which slot the ring had put it in, which nothing reads.
    pub capabilities: Vec<String>,
    /// The policy digests, in first-seen order, on the same terms as [`Assembled::capabilities`].
    pub digests: Vec<[u8; 32]>,
    /// Cycles whose four messages did not arrive, or did not agree about when they happened.
    pub incomplete: Vec<Incomplete>,
    /// Per field, how many cycles did not carry it.
    pub absent: Absent,
}

/// Put the cycles in `recording` back together.
///
/// `Record::sequence` is the cycle's **ordinal in this file**, one-based, and not the sequence the
/// ring gave the record. The file does not carry that: the metadata names the window's first and
/// last sequence and its holes, but a message cannot say which record it was. A caller reporting a
/// divergence should lead with [`Cycle::tick`], which means the same thing on both sides.
pub fn assemble(recording: &Recording) -> Result<Assembled, AssembleError> {
    let mut out = Assembled::default();

    let states = by_sequence(recording, id::STATE);
    let desired = by_sequence(recording, id::DESIRED);
    let verdicts = by_sequence(recording, id::VERDICT);
    let commands = by_sequence(recording, id::COMMAND);

    // Cursors rather than a shared index. A file truncated between two of a cycle's four messages
    // leaves the channels one apart from each other for the rest of the window, and a positional
    // join would then assemble every later cycle out of two different ones — each field plausible,
    // the whole thing wrong. Matching on the sequence costs one cursor per channel and turns that
    // into one reported gap.
    let mut at_desired = 0usize;
    let mut at_verdict = 0usize;
    let mut at_command = 0usize;

    for state in states.iter() {
        let cycle_no = state.sequence;
        let (Some(want), Some(saw), Some(sent)) = (
            seek(&desired, &mut at_desired, cycle_no),
            seek(&verdicts, &mut at_verdict, cycle_no),
            seek(&commands, &mut at_command, cycle_no),
        ) else {
            out.incomplete.push(Incomplete {
                cycle: cycle_no,
                reason: "one of the cycle's four messages is not in the file".to_string(),
            });
            continue;
        };

        let state_doc = object_of(state)?;
        let want_doc = object_of(want)?;
        let saw_doc = object_of(saw)?;
        let sent_doc = object_of(sent)?;

        let mono_ns = integer(&state_doc, "mono_ns").unwrap_or(0.0) as u64;
        let disagreed = [&want_doc, &saw_doc, &sent_doc]
            .iter()
            .any(|doc| integer(doc, "mono_ns").unwrap_or(0.0) as u64 != mono_ns);
        if disagreed {
            out.incomplete.push(Incomplete {
                cycle: cycle_no,
                reason: "the four messages disagree about mono_ns, so they are not one cycle"
                    .to_string(),
            });
            continue;
        }

        // The channel table comes out of the values array, which carries a name per index. Every
        // cycle in a ring has the same one; a file where that is untrue did not come from a ring.
        let names = channel_names(&want_doc);
        if out.channels.is_empty() {
            out.channels = names.clone();
        } else if out.channels != names && !names.is_empty() {
            return Err(AssembleError::ChannelsChanged { cycle: cycle_no });
        }

        let (desired_values, mask) = values_and_mask(&want_doc);
        let (command_values, _) = values_and_mask(&sent_doc);

        // Absent is not null. `get` answers `Some(Json::Null)` for a key the writer wrote with
        // nothing in it and `None` for a key it never wrote, and the whole point of counting here
        // is to keep a recording that predates a field apart from a cycle that had nothing to put
        // in one.
        if want_doc.get("capability").is_none() {
            out.absent.capability += 1;
        }
        if state_doc.get("nearest_person_m").is_none() {
            out.absent.nearest_person_m += 1;
        }
        if state_doc.get("read_result").is_none() {
            out.absent.read_result += 1;
        }

        let capability = match want_doc.get("capability").and_then(Json::as_str) {
            Some(token) => intern(&mut out.capabilities, token),
            None => NO_CAPABILITY,
        };
        let policy = match want_doc.get("policy_digest").and_then(Json::as_str) {
            Some(text) => match unhex(text) {
                Some(digest) => intern_digest(&mut out.digests, digest),
                None => 0,
            },
            None => 0,
        };

        let read = match state_doc.get("read_result") {
            None | Some(Json::Null) => ReadOutcome::Unrecorded,
            Some(value) => match value.as_f64() {
                Some(0.0) => ReadOutcome::Ok,
                Some(code) => ReadOutcome::Failed(code as i32),
                None => ReadOutcome::Unrecorded,
            },
        };

        let clamped = clamped_flags(&saw_doc, &out.channels);

        out.records.push(Record {
            sequence: cycle_no as u64,
            mono_ns,
            body: Body::Cycle(Cycle {
                tick: integer(&want_doc, "tick").unwrap_or(0.0) as u64,
                observation: want_doc
                    .get("observation")
                    .and_then(Json::as_str)
                    .and_then(unhex)
                    .unwrap_or([0u8; 32]),
                policy,
                state_flags: integer(&state_doc, "flags").unwrap_or(0.0) as u16,
                fault_code: integer(&state_doc, "fault_code").unwrap_or(0.0) as u32,
                rhal_result: integer(&sent_doc, "rhal_result").unwrap_or(0.0) as i32,
                read,
                state_seq: integer(&state_doc, "seq").unwrap_or(0.0) as u32,
                state_stamp_ns: integer(&state_doc, "stamp_ns").unwrap_or(0.0) as u64,
                command_stamp_ns: integer(&want_doc, "stamp_ns").unwrap_or(0.0) as u64,
                command_seq: integer(&want_doc, "seq").unwrap_or(0.0) as u32,
                mode: mode_of(&want_doc),
                advisory: saw_doc
                    .get("advisory")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                response: saw_doc
                    .get("response")
                    .and_then(Json::as_str)
                    .and_then(FaultResponse::parse),
                tripped: tripped_guards(&saw_doc),
                capability,
                nearest_person_m: maybe_f32(&state_doc, "nearest_person_m"),
                base_pose: fixed7(&state_doc, "base_pose"),
                base_twist: fixed6(&state_doc, "base_twist"),
                desired: desired_values.iter().map(|value| *value as f32).collect(),
                command: command_values,
                mask,
                clamped,
                position: floats(&state_doc, "position_rad"),
                velocity: floats(&state_doc, "velocity_rad_s"),
                effort: floats(&state_doc, "effort_nm"),
            }),
        });
    }

    Ok(out)
}

/// One channel's messages, in sequence order.
///
/// Sorted rather than assumed sorted: the seal writes them in order, but this reader opens files it
/// did not write, and a join that silently mismatched on a file whose messages had been reordered
/// would be the quietest possible way to produce a wrong cycle.
fn by_sequence(recording: &Recording, channel: u16) -> Vec<&Message> {
    let mut held: Vec<&Message> = recording.on(channel).collect();
    held.sort_by_key(|message| message.sequence);
    held
}

/// The message with sequence `wanted`, advancing `cursor` past everything before it.
///
/// `None` when this channel has no message for that cycle, which is what makes a missing quarter of
/// a cycle a reported gap rather than a silent shift.
fn seek<'a>(held: &[&'a Message], cursor: &mut usize, wanted: u32) -> Option<&'a Message> {
    while *cursor < held.len() && held[*cursor].sequence < wanted {
        *cursor += 1;
    }
    match held.get(*cursor) {
        Some(message) if message.sequence == wanted => Some(message),
        _ => None,
    }
}

fn object_of(message: &Message) -> Result<Json, AssembleError> {
    let parsed = Json::parse(&message.payload).map_err(|_| AssembleError::NotJson {
        channel: message.channel_id,
        sequence: message.sequence,
    })?;
    if parsed.as_obj().is_none() {
        return Err(AssembleError::NotJson {
            channel: message.channel_id,
            sequence: message.sequence,
        });
    }
    Ok(parsed)
}

fn integer(document: &Json, key: &str) -> Option<f64> {
    document.get(key).and_then(Json::as_f64)
}

/// A number the file may have written as `null`, which is §24's `NaN` in a format that has no
/// `NaN`. Absent reads the same way, because a field nobody wrote is not a measurement either.
fn maybe_f32(document: &Json, key: &str) -> f32 {
    match document.get(key).and_then(Json::as_f64) {
        Some(value) => value as f32,
        None => f32::NAN,
    }
}

fn floats(document: &Json, key: &str) -> Vec<f32> {
    match document.get(key).and_then(Json::as_arr) {
        Some(items) => items
            .iter()
            .map(|item| item.as_f64().map(|value| value as f32).unwrap_or(f32::NAN))
            .collect(),
        None => Vec::new(),
    }
}

fn fixed7(document: &Json, key: &str) -> [f32; 7] {
    let mut out = [f32::NAN; 7];
    let held = floats(document, key);
    for (slot, value) in out.iter_mut().zip(held.iter()) {
        *slot = *value;
    }
    out
}

fn fixed6(document: &Json, key: &str) -> [f32; 6] {
    let mut out = [f32::NAN; 6];
    let held = floats(document, key);
    for (slot, value) in out.iter_mut().zip(held.iter()) {
        *slot = *value;
    }
    out
}

fn channel_names(document: &Json) -> Vec<String> {
    match document.get("values").and_then(Json::as_arr) {
        Some(items) => items
            .iter()
            .map(|item| {
                item.get("channel")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
        None => Vec::new(),
    }
}

/// The values and the hold mask, which the file writes as one array of triples.
///
/// §24's mask is *0 means hold*, and the seal writes the boolean rather than the byte — so a
/// non-hold byte comes back as 1 whatever it was. Nothing reads a mask byte's magnitude; the guard
/// that consumes it asks whether it is zero.
fn values_and_mask(document: &Json) -> (Vec<f64>, Vec<u8>) {
    let mut values = Vec::new();
    let mut mask = Vec::new();
    if let Some(items) = document.get("values").and_then(Json::as_arr) {
        for item in items {
            values.push(item.get("value").and_then(Json::as_f64).unwrap_or(f64::NAN));
            let hold = item.get("hold").and_then(Json::as_bool).unwrap_or(false);
            mask.push(if hold { 0 } else { 1 });
        }
    }
    (values, mask)
}

/// The clamp flags, one per channel.
///
/// The file lists only the channels that were reduced, which is the right thing to write — a
/// hundred `false`s per cycle is not evidence — and the inverse is this: a dense vector as wide as
/// the channel table, with the named ones set.
fn clamped_flags(document: &Json, channels: &[String]) -> Vec<bool> {
    let mut flags = alloc::vec![false; channels.len()];
    if let Some(items) = document.get("clamped").and_then(Json::as_arr) {
        for item in items {
            let Some(name) = item.get("channel").and_then(Json::as_str) else {
                continue;
            };
            if let Some(index) = channels.iter().position(|held| held == name) {
                flags[index] = true;
            }
        }
    }
    flags
}

fn tripped_guards(document: &Json) -> Vec<Guard> {
    match document.get("tripped").and_then(Json::as_arr) {
        Some(items) => items
            .iter()
            .filter_map(|item| item.as_str().and_then(Guard::parse))
            .collect(),
        None => Vec::new(),
    }
}

fn mode_of(document: &Json) -> u8 {
    match document.get("mode").and_then(Json::as_str) {
        Some("position") => 0,
        Some("velocity") => 1,
        Some("torque") => 2,
        _ => MODE_UNKNOWN,
    }
}

fn intern(table: &mut Vec<String>, token: &str) -> u16 {
    if let Some(index) = table.iter().position(|held| held == token) {
        return index as u16;
    }
    table.push(token.to_string());
    (table.len() - 1) as u16
}

fn intern_digest(table: &mut Vec<[u8; 32]>, digest: [u8; 32]) -> u16 {
    if let Some(index) = table.iter().position(|held| *held == digest) {
        return index as u16;
    }
    table.push(digest);
    (table.len() - 1) as u16
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let high = nibble(bytes[index * 2])?;
        let low = nibble(bytes[index * 2 + 1])?;
        *slot = (high << 4) | low;
    }
    Some(out)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
