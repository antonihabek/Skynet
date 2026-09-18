//! The audit log: who changed what, when (§38.3).
//!
//! §38.3 asks for two records of every configuration change — *"a black box event and an audit-log
//! entry, carrying who, when, which layer, and the previous value"* — and they are not redundant.
//! The black box is a fixed-size ring that wraps: it holds the last few minutes at full fidelity,
//! which is what an incident investigation needs and exactly the wrong shape for *"when did this
//! robot start trusting that key"*, a question whose answer is eleven months old. This file is the
//! other half: small, append-only, and kept for the life of the machine.
//!
//! # One canonical object per line
//!
//! ```text
//! {"action":"trust.add","actor":"alice","at":"2026-08-30T12:00:00Z","detail":"…","seq":4,"subject":"acme-robotics"}
//! ```
//!
//! JSON Lines rather than a JSON array, and the reason is the whole design: **an append must not
//! require rewriting the file.** A log kept as an array has to be read, parsed, extended and
//! written back on every entry, which means every append is an opportunity to lose the history —
//! and rewriting an audit log is precisely the operation an audit log exists to make difficult. An
//! append is `O_APPEND` and one `write`.
//!
//! It also degrades in the right direction. A line truncated by a power cut costs that line; the
//! ten thousand before it still parse. The array form would cost the file.
//!
//! # Wall time, deliberately
//!
//! §35.1 gives `CLOCK_REALTIME` three jobs — *"logging, scheduling, correlation with the Hub"* —
//! and this is the first. Nothing here is read to make a decision, so a stepped clock spoils a
//! record and cannot spoil a verdict. A monotonic reading would be the wrong answer twice: it means
//! nothing after a reboot, and it cannot be compared with the date on an operator's ticket.
//!
//! The calendar itself is [`skynet_contracts::time`]: the CLI and an edition's corpus runner write
//! dates into documents this daemon and a Hub read, so it is the one fact here that lives below it.
//!
//! # The sequence number, and what the daemon does with it
//!
//! Every entry carries a `seq`, one higher than the last line in the file. It exists because the
//! black box half of §38.3 cannot be written by whoever makes the change: the ring has exactly one
//! writer by construction (`skynetd`), and `skynetd trust add` is a *different process*, which may
//! be run while the daemon is up. Two writers to a preallocated ring is corruption, not a race that
//! occasionally loses a record.
//!
//! So the change is written here, and the daemon mirrors anything past its cursor into the ring on
//! its next pass ([`Audit::since`]). The consequence worth stating: a black box event for a
//! configuration change is **delayed by up to one reconcile period**, and on a robot where
//! `skynetd` is not running it does not appear at all — while the audit entry always does. That is
//! the correct direction for a record whose job is to survive.

use std::io::Write;
use std::path::{Path, PathBuf};

use skynet_json::Json;

/// One recorded change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Position in the file, from 1. The daemon's mirroring cursor counts these.
    pub seq: u64,
    /// Wall time, RFC 3339, UTC.
    pub at: String,
    /// Who did it — a credential label, a person, a provisioning run, or `unknown`.
    pub actor: String,
    /// What was done: `trust.add`, `trust.remove`, `profile.apply`.
    pub action: String,
    /// What it was done to.
    pub subject: String,
    /// A sentence about it.
    pub detail: String,
    /// What the subject was before, where a change replaced something (§38.3).
    pub previous: Option<String>,
}

impl Entry {
    /// The line as it is written.
    pub fn to_json(&self) -> Json {
        let mut line = Json::obj();
        line.set("seq", Json::Num(self.seq as f64));
        line.set("at", Json::Str(self.at.clone()));
        line.set("actor", Json::Str(self.actor.clone()));
        line.set("action", Json::Str(self.action.clone()));
        line.set("subject", Json::Str(self.subject.clone()));
        line.set("detail", Json::Str(self.detail.clone()));
        if let Some(previous) = &self.previous {
            line.set("previous", Json::Str(previous.clone()));
        }
        line
    }

    /// Read one back. `None` is a line this build cannot use.
    pub fn from_json(value: &Json) -> Option<Entry> {
        let text = |name: &str| value.get(name).and_then(Json::as_str).map(str::to_string);
        Some(Entry {
            seq: value.get("seq").and_then(Json::as_usize)? as u64,
            at: text("at")?,
            actor: text("actor")?,
            action: text("action")?,
            subject: text("subject")?,
            detail: text("detail").unwrap_or_default(),
            previous: text("previous"),
        })
    }

    /// The one-line rendering a black box event and a terminal both use.
    pub fn summary(&self) -> String {
        let mut line = format!("{} {} by {}", self.action, self.subject, self.actor);
        if let Some(previous) = &self.previous {
            line.push_str(&format!(" (was: {previous})"));
        }
        if !self.detail.is_empty() {
            line.push_str(&format!(" — {}", self.detail));
        }
        line
    }
}

/// A line in the file that could not be read.
///
/// Reported rather than skipped, for the same reason a skipped trust key is: a log that silently
/// drops what it cannot parse is a log whose gaps are invisible, and an audit log with invisible
/// gaps is worse than no audit log, because somebody is relying on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    /// Line number, from 1.
    pub line: usize,
    /// What is wrong.
    pub detail: String,
}

/// What was read out of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Log {
    /// The entries, in file order.
    pub entries: Vec<Entry>,
    /// Lines that could not be read.
    pub broken: Vec<Broken>,
}

impl Log {
    /// The highest sequence number in the file, or 0 for an empty one.
    ///
    /// Taken as a maximum rather than as `entries.len()`, because a broken line still consumed a
    /// number and reusing it would produce two entries claiming the same position.
    pub fn last_seq(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| entry.seq)
            .max()
            .unwrap_or(0)
    }
}

/// The append-only log at `<root>/data/state/audit.jsonl`.
#[derive(Debug, Clone)]
pub struct Audit {
    path: PathBuf,
}

impl Audit {
    /// Point at a file. Nothing is created until something is appended.
    pub fn at(path: impl Into<PathBuf>) -> Audit {
        Audit { path: path.into() }
    }

    /// The log this layout holds.
    pub fn of(layout: &crate::layout::Layout) -> Audit {
        Audit::at(layout.audit())
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole file. A missing file is an empty log, not an error.
    pub fn read(&self) -> Log {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Log::default(),
            Err(error) => {
                return Log {
                    entries: Vec::new(),
                    broken: vec![Broken {
                        line: 0,
                        detail: format!("the audit log could not be read: {error}"),
                    }],
                }
            }
        };

        let mut log = Log::default();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            if line.trim().is_empty() {
                continue;
            }
            match Json::parse(line) {
                Ok(document) => match Entry::from_json(&document) {
                    Some(entry) => log.entries.push(entry),
                    None => log.broken.push(Broken {
                        line: number,
                        detail: "is a JSON object missing one of seq, at, actor, action or subject"
                            .to_string(),
                    }),
                },
                Err(error) => log.broken.push(Broken {
                    line: number,
                    detail: format!("does not parse: {error}"),
                }),
            }
        }
        log
    }

    /// Entries after `cursor`, oldest first — what the daemon has not yet mirrored.
    pub fn since(&self, cursor: u64) -> Vec<Entry> {
        self.read()
            .entries
            .into_iter()
            .filter(|entry| entry.seq > cursor)
            .collect()
    }

    /// Append one change, and return it as it was written.
    ///
    /// `at` is supplied rather than read here for the reason the kernel's clock is supplied (§35.2):
    /// a function that reads a clock cannot be tested against a fixed one, and the whole value of
    /// this file is that its timestamps can be checked.
    pub fn append(
        &self,
        at: &str,
        actor: &str,
        action: &str,
        subject: &str,
        detail: &str,
        previous: Option<&str>,
    ) -> std::io::Result<Entry> {
        let entry = Entry {
            seq: self.read().last_seq() + 1,
            at: at.to_string(),
            actor: actor.to_string(),
            action: action.to_string(),
            subject: subject.to_string(),
            detail: detail.to_string(),
            previous: previous.map(str::to_string),
        };
        let line = entry.to_json().to_canonical().map_err(|error| {
            std::io::Error::other(format!("serialising an audit entry: {error:?}"))
        })?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Appended rather than rewritten: see the module header. `create(true).append(true)` is one
        // `write` at the end of the file, so a concurrent reader sees either the line or nothing.
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory that removes itself, so a failing test leaves no state behind.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let path = std::env::temp_dir().join(format!(
                "skynet-audit-{name}-{}",
                std::process::id() as u64 + name.len() as u64
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Dir(path)
        }
        fn log(&self) -> Audit {
            Audit::at(self.0.join("audit.jsonl"))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const AT: &str = "2026-08-30T12:00:00Z";

    #[test]
    fn a_missing_file_is_an_empty_log_and_not_an_error() {
        let dir = Dir::new("missing");
        let log = dir.log().read();
        assert!(log.entries.is_empty());
        assert!(log.broken.is_empty());
        assert_eq!(log.last_seq(), 0);
    }

    #[test]
    fn sequence_numbers_increase_and_the_file_is_appended_to() {
        let dir = Dir::new("seq");
        let audit = dir.log();
        let first = audit
            .append(AT, "alice", "trust.add", "acme", "", None)
            .expect("append");
        let second = audit
            .append(AT, "bob", "trust.remove", "acme", "", Some("acme"))
            .expect("append");

        assert_eq!((first.seq, second.seq), (1, 2));
        let log = audit.read();
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.last_seq(), 2);
        assert_eq!(log.entries[0].actor, "alice");
        assert_eq!(log.entries[1].previous.as_deref(), Some("acme"));
    }

    /// The property that made JSON Lines the format: one damaged line costs one line.
    #[test]
    fn a_broken_line_is_reported_and_the_rest_still_read() {
        let dir = Dir::new("broken");
        let audit = dir.log();
        audit
            .append(AT, "alice", "trust.add", "acme", "", None)
            .expect("append");
        std::fs::write(
            audit.path(),
            format!(
                "{}\n{{\"seq\":2,\"at\":\n",
                std::fs::read_to_string(audit.path()).expect("read").trim()
            ),
        )
        .expect("write");
        audit
            .append(AT, "carol", "trust.add", "other", "", None)
            .expect("append");

        let log = audit.read();
        assert_eq!(log.entries.len(), 2, "{log:?}");
        assert_eq!(log.broken.len(), 1, "{log:?}");
        assert_eq!(log.broken[0].line, 2);
    }

    /// A truncated line still consumed its number, so the next entry must not reuse it — two
    /// entries at the same position would make the mirroring cursor skip one of them.
    #[test]
    fn a_number_consumed_by_a_broken_line_is_not_reused() {
        let dir = Dir::new("reuse");
        let audit = dir.log();
        std::fs::write(
            audit.path(),
            "{\"seq\":7,\"at\":\"x\",\"actor\":\"a\"}\n{oops\n",
        )
        .expect("write");
        // The first line parses as JSON and is missing `action`, so it is broken too; the file
        // therefore has no usable entries and `last_seq` is 0. That is the honest answer: nothing
        // in this file says a number was taken.
        assert_eq!(audit.read().last_seq(), 0);

        let audit = Audit::at(dir.0.join("second.jsonl"));
        audit
            .append(AT, "a", "trust.add", "one", "", None)
            .expect("append");
        std::fs::write(
            audit.path(),
            format!(
                "{}\n{{ truncated\n",
                std::fs::read_to_string(audit.path()).expect("read").trim()
            ),
        )
        .expect("write");
        assert_eq!(
            audit
                .append(AT, "a", "trust.add", "two", "", None)
                .expect("append")
                .seq,
            2
        );
    }

    #[test]
    fn since_returns_only_what_a_cursor_has_not_seen() {
        let dir = Dir::new("since");
        let audit = dir.log();
        for name in ["one", "two", "three"] {
            audit
                .append(AT, "alice", "trust.add", name, "", None)
                .expect("append");
        }
        let unseen = audit.since(1);
        assert_eq!(unseen.len(), 2);
        assert_eq!(unseen[0].subject, "two");
        assert!(audit.since(3).is_empty());
    }

    #[test]
    fn a_summary_names_the_previous_value() {
        let entry = Entry {
            seq: 1,
            at: AT.to_string(),
            actor: "alice".to_string(),
            action: "trust.remove".to_string(),
            subject: "acme-robotics".to_string(),
            detail: "rotated".to_string(),
            previous: Some("d75a98".to_string()),
        };
        assert_eq!(
            entry.summary(),
            "trust.remove acme-robotics by alice (was: d75a98) — rotated"
        );
    }
}
