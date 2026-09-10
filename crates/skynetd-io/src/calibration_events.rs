//! §45.4's event triggers, on a running robot.
//!
//! > *"The event triggers are the ones that matter. A robot that has just hit something has, by
//! > definition, possibly moved its cameras, and continuing to trust yesterday's extrinsics is
//! > exactly the wrong response to an impact."*
//!
//! [`skynet_contracts::calibration`] has decided this since it was written: a record carries
//! `events_since`, a policy carries `invalidating_events`, and one matching the other expires the
//! record outright. What was missing is the only part that makes any of it reachable — **nothing on
//! a running robot could record an event**. `events_since` came from the stored record and from
//! nowhere else, so the field could only change when somebody rewrote a signed document, which is
//! to say: a collision that happened this morning invalidated nothing at all.
//!
//! This is the journal that fills it. `/data/state/calibration/events.jsonl`, append-only, one
//! canonical JSON object per line — the shape §38.3 chose for the audit log and for the same
//! reason: **an append must not require rewriting the file**, and rewriting a record of impacts is
//! the operation such a record exists to make difficult.
//!
//! # Why unsigned is safe here, and it is the same argument §38.2 makes
//!
//! Everything else that reaches a guard is signed. This is not, and what makes that safe is a
//! property of the direction it can push: **an event can only invalidate a calibration, never
//! validate one.** `evaluate` reaches `Validity::Expired` on a match and there is no path by which
//! a recorded event makes a record believed. So the worst a forged or corrupted journal can do is
//! retire guards that would otherwise run, which is a robot that is more cautious than it needs to
//! be — the same shape as §38.2's session override, where the worst a forged one can do is slow the
//! robot down.
//!
//! The reverse would not be safe and is worth naming so nobody adds it: a journal that could
//! *clear* an event would be an unsigned path to making a stale calibration believed again, which
//! is precisely the failure §45.1 opens with — a `proximity` guard measuring to the wrong place
//! while reporting perfectly healthy.
//!
//! # Since means since, and that is what prunes this file
//!
//! An event only counts against a record it is **newer than**. A collision recorded in March does
//! not invalidate a calibration measured in April, and reading it as though it did would make
//! re-measuring impossible: the set would expire again the instant it was written.
//!
//! That is also why nothing here has a delete. Re-measuring a set moves its `at_s` forward and the
//! old events stop counting on their own, which is the correct lifecycle for a record of things
//! that happened — they did happen, and the file goes on saying so.
//!
//! # The vocabulary is closed
//!
//! Only [`INVALIDATING_EVENTS`] are accepted, and the hazard that closes it is specific rather than
//! tidy-minded: a `collison` recorded with a typo matches no policy, invalidates nothing, and
//! leaves an operator believing they have flagged an impact. A refusal naming the five is the
//! difference between a mistake somebody sees and a robot quietly trusting a disturbed extrinsic.

use skynet_contracts::calibration::INVALIDATING_EVENTS;
use skynet_json::Json;

use crate::layout::Layout;

/// The most entries read back from the journal.
///
/// A bound rather than a promise the file stays small: events are rare — a collision, a tool change
/// — so this is reached only by something writing them in a loop, and that is exactly when a
/// calibration report must still answer. The newest are kept, because an event older than every
/// stored record already counts against nothing.
pub const MAX_ENTRIES: usize = 1000;

/// One recorded event.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Which event, from [`INVALIDATING_EVENTS`].
    pub event: String,
    /// When, in seconds on the robot's wall clock — the same clock a record's conditions use, so
    /// the two are comparable without conversion.
    pub at_s: f64,
    /// Who recorded it. §38.3's provenance: a role, never a person.
    pub recorded_by: String,
    /// What happened, in the operator's own words. Optional.
    pub note: Option<String>,
}

impl Entry {
    fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("event", Json::from(self.event.as_str()));
        out.set("at_s", Json::Num(self.at_s));
        out.set("recorded_by", Json::from(self.recorded_by.as_str()));
        if let Some(note) = &self.note {
            out.set("note", Json::from(note.as_str()));
        }
        out
    }

    fn from_json(document: &Json) -> Option<Entry> {
        Some(Entry {
            event: document.get("event").and_then(Json::as_str)?.to_string(),
            at_s: document.get("at_s").and_then(Json::as_f64)?,
            recorded_by: document
                .get("recorded_by")
                .and_then(Json::as_str)
                // §38.3: provenance is recorded, never inferred. A line written by hand has none,
                // and the honest report of that is a word rather than a name nobody typed.
                .unwrap_or("unrecorded")
                .to_string(),
            note: document
                .get_present("note")
                .and_then(Json::as_str)
                .map(str::to_string),
        })
    }
}

/// What reading the journal found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Journal {
    /// The entries, oldest first.
    pub entries: Vec<Entry>,
    /// Lines that could not be read.
    ///
    /// Reported and never skipped, for §38.3's reason about the audit log: a record with invisible
    /// gaps is worse than no record, because somebody is relying on it. Here it is worse still —
    /// a dropped line is an impact that stops invalidating anything.
    pub unreadable: Vec<String>,
}

impl Journal {
    /// The events recorded **since** a moment, newest last.
    ///
    /// `None` for `since_s` means a record that does not say when it was taken. Every event counts
    /// then, and that is the conservative direction: a record whose date nobody wrote down cannot
    /// be shown to predate an impact, and §45.4 is a section about not trusting things that might
    /// have been disturbed.
    pub fn since(&self, since_s: Option<f64>) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| match since_s {
                Some(at) => entry.at_s > at,
                None => true,
            })
            .map(|entry| entry.event.clone())
            .collect()
    }
}

/// Read the journal.
///
/// Total: a missing file is an empty journal, which is the ordinary state of a robot nothing has
/// happened to.
pub fn read(layout: &Layout) -> Journal {
    let path = layout.calibration_events();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Journal::default(),
        Err(error) => {
            return Journal {
                entries: Vec::new(),
                unreadable: vec![format!("{path:?} could not be read: {error}")],
            }
        }
    };

    let mut journal = Journal::default();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match Json::parse(line).ok().as_ref().and_then(Entry::from_json) {
            Some(entry) => journal.entries.push(entry),
            None => journal.unreadable.push(format!(
                "line {} of {path:?} is not a calibration event and was not counted",
                number + 1
            )),
        }
    }

    // The newest, when there are more than anybody needs. An event older than every stored record
    // already counts against nothing, so dropping from the front loses the entries that were
    // already inert.
    if journal.entries.len() > MAX_ENTRIES {
        let excess = journal.entries.len() - MAX_ENTRIES;
        journal.entries.drain(..excess);
        journal.unreadable.push(format!(
            "{excess} older event(s) in {path:?} were not read; this report covers the newest \
             {MAX_ENTRIES}"
        ));
    }
    journal
}

/// Why an event was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// Not one of [`INVALIDATING_EVENTS`].
    UnknownEvent(String),
    /// It could not be appended.
    Write(String),
}

impl Refusal {
    /// The sentence, which names the fix (§64).
    pub fn detail(&self) -> String {
        match self {
            Refusal::UnknownEvent(said) | Refusal::Write(said) => said.clone(),
        }
    }
}

/// Record one event.
///
/// Appends. There is deliberately no way to remove one — see this module's documentation: a journal
/// that could clear an event would be an unsigned path to making a stale calibration believed.
pub fn record(layout: &Layout, entry: Entry) -> Result<Entry, Refusal> {
    if !INVALIDATING_EVENTS.contains(&entry.event.as_str()) {
        return Err(Refusal::UnknownEvent(format!(
            "`{}` is not an event that invalidates a calibration. §45.4 names five, and one \
             spelled any other way matches no record's policy and would invalidate nothing: {}.",
            entry.event,
            INVALIDATING_EVENTS.join(", ")
        )));
    }

    let path = layout.calibration_events();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Refusal::Write(format!("{parent:?} could not be created: {error}")))?;
    }
    let line = entry
        .to_json()
        .to_canonical()
        .map_err(|error| Refusal::Write(format!("the event could not be encoded: {error}")))?;

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| Refusal::Write(format!("{path:?} could not be opened: {error}")))?;
    writeln!(file, "{line}")
        .map_err(|error| Refusal::Write(format!("{path:?} could not be appended to: {error}")))?;
    Ok(entry)
}

#[cfg(test)]
#[path = "calibration_events_tests.rs"]
mod tests;
