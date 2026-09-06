//! DNS-SD's TXT record convention (RFC 6763 §6).
//!
//! A TXT record's payload is **not** a string and not a flat key/value blob. It is a sequence of
//! length-prefixed byte strings, each of which is a `key=value` pair. Reading it as text and
//! splitting on newlines — which is how it is usually got wrong — produces a parser that works
//! against whichever responder was tested and fails against the next one.
//!
//! # Where this is lenient, and why that is not a contradiction
//!
//! [`crate::wire`] refuses a malformed message whole. This ignores a malformed entry and keeps the
//! rest, which looks like the opposite discipline and is the same one applied to a different blast
//! radius:
//!
//! * a bad length prefix in the wire format can walk the parser into the middle of another record
//!   and yield a *different, well-formed-looking* answer. Refusal is the only safe response.
//! * a TXT entry that is not `key=value` affects exactly one key of one instance, and RFC 6763 §6.4
//!   says in as many words to ignore it. Refusing the record instead would let any host on the LAN
//!   erase a robot from an operator's list by appending one junk byte to a packet.
//!
//! **First occurrence wins** for a repeated key, which is also the RFC's rule and is the half worth
//! stating: it means an entry appended to a record cannot override one already in it.
//!
//! # Values are bytes
//!
//! RFC 6763 §6.5 is explicit that a value is opaque binary. [`Txt::get_str`] is offered for the
//! keys this crate defines — which are ASCII by construction — and returns `None` rather than a
//! replacement character for anything else, because a robot identifier that has been lossily
//! converted is an identifier that will not match the one the Hub holds.
//!
//! # A flag is not an empty value
//!
//! `foo` and `foo=` are different states in RFC 6763: the first is an attribute present with no
//! value, the second an attribute whose value is the empty string. They are kept apart
//! ([`Value::Flag`] against [`Value::Bytes`]) because collapsing them is how a boolean gets read
//! as a string somewhere downstream and quietly becomes true.

use crate::wire::{Record, RecordData};

/// The largest single entry, fixed by the one-byte length prefix.
pub const MAX_ENTRY: usize = 255;

/// The largest whole TXT payload this will build.
///
/// RFC 6763 §6.2 asks that a record fit comfortably in one packet alongside its SRV and address
/// records. A Skynet advertisement is a few dozen bytes and this bound exists so that a caller who
/// starts putting state into a discovery record hits an error rather than a fragmented answer —
/// see [`crate::service`] for why nothing should be putting state in one at all.
pub const MAX_PAYLOAD: usize = 1300;

/// What a key carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// The key appeared with no `=` — an attribute that is present and has no value.
    Flag,
    /// The key appeared with `=`. The bytes may be empty, which is a distinct state from [`Self::Flag`].
    Bytes(Vec<u8>),
}

/// A parsed TXT record, in the order its entries appeared.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Txt {
    entries: Vec<(Vec<u8>, Value)>,
}

impl Txt {
    /// An empty record.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a TXT payload.
    ///
    /// Never fails. A truncated final entry, an entry beginning with `=`, and a zero-length entry
    /// are each skipped per RFC 6763 §6.1 and §6.4.
    pub fn parse(payload: &[u8]) -> Self {
        let mut entries: Vec<(Vec<u8>, Value)> = Vec::new();
        let mut at = 0;

        while at < payload.len() {
            let len = payload[at] as usize;
            at += 1;
            let Some(entry) = payload.get(at..at + len) else {
                // A length prefix promising more than the record holds. The entries already read
                // are intact and are kept; there is nothing after this to resynchronise to.
                break;
            };
            at += len;

            // "Empty strings are ignored" — and a leading '=' is an entry with an empty key,
            // which §6.4 says to ignore rather than to treat as a key of "".
            if entry.is_empty() || entry[0] == b'=' {
                continue;
            }

            let (key, value) = match entry.iter().position(|byte| *byte == b'=') {
                Some(at) => (&entry[..at], Value::Bytes(entry[at + 1..].to_vec())),
                None => (entry, Value::Flag),
            };

            // First occurrence wins, so an appended duplicate cannot override an earlier key.
            if !entries.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
                entries.push((key.to_vec(), value));
            }
        }

        Self { entries }
    }

    /// Read the TXT record of a [`Record`], if it is one.
    pub fn of(record: &Record) -> Option<Self> {
        match &record.data {
            RecordData::Txt(payload) => Some(Self::parse(payload)),
            _ => None,
        }
    }

    /// Look a key up, ASCII case-insensitively (RFC 6763 §6.4).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key.as_bytes()))
            .map(|(_, value)| value)
    }

    /// A key's value as text, or `None` if it is absent, a flag, or not UTF-8.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
            Value::Flag => None,
        }
    }

    /// Whether a key is present in any form.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Every key, in order.
    pub fn keys(&self) -> impl Iterator<Item = &[u8]> {
        self.entries.iter().map(|(key, _)| key.as_slice())
    }

    /// How many entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add `key=value`. A repeated key is ignored, matching the parse rule.
    pub fn set(&mut self, key: &str, value: &str) -> &mut Self {
        self.push(key, Value::Bytes(value.as_bytes().to_vec()))
    }

    /// Add a valueless key.
    pub fn flag(&mut self, key: &str) -> &mut Self {
        self.push(key, Value::Flag)
    }

    fn push(&mut self, key: &str, value: Value) -> &mut Self {
        if !self
            .entries
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(key.as_bytes()))
        {
            self.entries.push((key.as_bytes().to_vec(), value));
        }
        self
    }

    /// Encode to a TXT payload.
    ///
    /// Returns `None` if any entry exceeds [`MAX_ENTRY`] or the whole exceeds [`MAX_PAYLOAD`] —
    /// a refusal rather than a truncation, because a discovery record missing the key that
    /// identified the robot is worse than no record.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for (key, value) in &self.entries {
            let mut entry = key.clone();
            if let Value::Bytes(bytes) = value {
                entry.push(b'=');
                entry.extend_from_slice(bytes);
            }
            if entry.len() > MAX_ENTRY {
                return None;
            }
            out.push(entry.len() as u8);
            out.extend_from_slice(&entry);
        }
        // RFC 6763 §6.1: an empty TXT record is a single zero byte, never zero bytes.
        if out.is_empty() {
            out.push(0);
        }
        if out.len() > MAX_PAYLOAD {
            return None;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(entries: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in entries {
            out.push(entry.len() as u8);
            out.extend_from_slice(entry.as_bytes());
        }
        out
    }

    #[test]
    fn a_txt_record_is_length_prefixed_entries_not_a_string() {
        let txt = Txt::parse(&payload(&["id=a1b2", "v=0.1.0"]));
        assert_eq!(txt.get_str("id"), Some("a1b2"));
        assert_eq!(txt.get_str("v"), Some("0.1.0"));
        assert_eq!(txt.len(), 2);
    }

    #[test]
    fn keys_are_ascii_case_insensitive() {
        let txt = Txt::parse(&payload(&["ID=a1b2"]));
        assert_eq!(txt.get_str("id"), Some("a1b2"));
        assert_eq!(txt.get_str("Id"), Some("a1b2"));
    }

    #[test]
    fn a_flag_and_an_empty_value_are_different_states() {
        let txt = Txt::parse(&payload(&["paired", "name="]));
        assert_eq!(txt.get("paired"), Some(&Value::Flag));
        assert_eq!(txt.get("name"), Some(&Value::Bytes(Vec::new())));
        // And a flag is not readable as a string, so it cannot become one downstream.
        assert_eq!(txt.get_str("paired"), None);
        assert_eq!(txt.get_str("name"), Some(""));
    }

    #[test]
    fn the_first_occurrence_of_a_key_wins_so_an_appended_entry_cannot_override_it() {
        let txt = Txt::parse(&payload(&["id=real", "id=spoofed", "ID=also-spoofed"]));
        assert_eq!(txt.get_str("id"), Some("real"));
        assert_eq!(txt.len(), 1);
    }

    #[test]
    fn a_value_containing_equals_keeps_it() {
        let txt = Txt::parse(&payload(&["note=a=b"]));
        assert_eq!(txt.get_str("note"), Some("a=b"));
    }

    // ── The lenient cases, each of which RFC 6763 requires to be a skip rather than a failure ──

    #[test]
    fn an_empty_entry_is_skipped_and_costs_nothing_around_it() {
        let txt = Txt::parse(&payload(&["a=1", "", "b=2"]));
        assert_eq!(txt.get_str("a"), Some("1"));
        assert_eq!(txt.get_str("b"), Some("2"));
        assert_eq!(txt.len(), 2);
    }

    #[test]
    fn an_entry_starting_with_equals_is_skipped_rather_than_becoming_an_empty_key() {
        let txt = Txt::parse(&payload(&["=orphan", "a=1"]));
        assert_eq!(txt.len(), 1);
        assert_eq!(txt.get_str("a"), Some("1"));
    }

    #[test]
    fn a_truncated_final_entry_keeps_everything_before_it() {
        // The last length prefix promises 40 bytes and four follow. A responder whose packet was
        // clipped must not cost an operator the robot's identifier.
        let mut bytes = payload(&["id=a1b2"]);
        bytes.push(40);
        bytes.extend_from_slice(b"v=0.");
        let txt = Txt::parse(&bytes);
        assert_eq!(txt.get_str("id"), Some("a1b2"));
        assert_eq!(txt.len(), 1);
    }

    #[test]
    fn a_non_utf8_value_reads_as_absent_rather_than_as_a_replacement_character() {
        let mut bytes = Vec::new();
        let entry = [b'i', b'd', b'=', 0xFF, 0xFE];
        bytes.push(entry.len() as u8);
        bytes.extend_from_slice(&entry);
        let txt = Txt::parse(&bytes);
        assert!(txt.has("id"));
        assert_eq!(txt.get_str("id"), None);
    }

    #[test]
    fn parsing_never_panics_on_arbitrary_bytes() {
        // Every one-byte prefix against a short buffer, which is where an off-by-one would land.
        for first in 0u8..=255 {
            for len in 0..6usize {
                let mut bytes = vec![first];
                bytes.extend(std::iter::repeat_n(b'a', len));
                let _ = Txt::parse(&bytes);
            }
        }
    }

    // ── Building ──

    #[test]
    fn a_built_record_round_trips_through_the_parser() {
        let mut txt = Txt::new();
        txt.set("id", "a1b2c3").set("v", "0.1.0").flag("unpaired");
        let parsed = Txt::parse(&txt.encode().unwrap());
        assert_eq!(parsed.get_str("id"), Some("a1b2c3"));
        assert_eq!(parsed.get_str("v"), Some("0.1.0"));
        assert_eq!(parsed.get("unpaired"), Some(&Value::Flag));
    }

    #[test]
    fn an_empty_record_encodes_as_one_zero_byte_not_as_nothing() {
        assert_eq!(Txt::new().encode(), Some(vec![0]));
    }

    #[test]
    fn an_oversized_entry_is_refused_rather_than_truncated() {
        let mut txt = Txt::new();
        txt.set("k", &"v".repeat(MAX_ENTRY));
        assert_eq!(txt.encode(), None);
    }

    #[test]
    fn an_oversized_record_is_refused_rather_than_fragmented() {
        let mut txt = Txt::new();
        for i in 0..40 {
            txt.set(&format!("k{i}"), &"v".repeat(200));
        }
        assert_eq!(txt.encode(), None);
    }

    #[test]
    fn setting_a_key_twice_keeps_the_first_like_the_parser_does() {
        let mut txt = Txt::new();
        txt.set("id", "first").set("id", "second");
        assert_eq!(txt.get_str("id"), Some("first"));
    }
}
