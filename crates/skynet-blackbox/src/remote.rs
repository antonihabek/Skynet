//! Which cycles ran on a remote policy's proposals, and from which session (§90.2).
//!
//! §90.2's last rule: *"the recording (§47.4) notes which cycles ran on remote proposals, with the
//! session id and the model digest."* A cycle record cannot carry either — its policy index points
//! into a header table no process on a robot grows mid-recording (§26.2), and a model digest a
//! session was repinned to is not a digest anything knew when the ring was created. So the
//! recording carries them as §26 events, [`EventCode::RemoteSession`], and a reader joins them to
//! the cycles here.
//!
//! # Three sentences, and why the join is by the command's stamp
//!
//! The policy host that holds the session says three things about it, each once:
//!
//! | Detail | When |
//! |---|---|
//! | `opened <digest>` | the session was established, serving that model |
//! | `proposing from command <seq> stamped <ns>` | its first proposal reached the command ring |
//! | `closed through command <seq> stamped <ns>` | its last proposal, said when the session ended |
//!
//! The supervisor carries them into the ring on its next pass, so they arrive **after** the cycles
//! they describe, by up to a reconcile period. That is why nothing here reads the order of records:
//! a cycle records the stamp of the command it executed (§24: when the policy produced it), and the
//! join is that stamp against the stamps the policy host reported. Sequence numbers are in the
//! sentences for a person reading the file, and are not the key, because a policy host that
//! restarts starts its sequence again at one while its clock carries on.
//!
//! Only one policy commands a body at a time (§25 has one loop, and the supervisor refuses a second
//! commanding package), so every command stamped inside a session's first and last proposal is that
//! session's — with one exception the join has to see: a policy host holding a stale body publishes
//! a **hold** of its own (every channel masked), which is not a proposal. A cycle whose command
//! commanded no channel is therefore never attributed.
//!
//! # The tolerance, and why it cannot confuse two sessions
//!
//! The kernel carries a command's stamp through milliseconds as a float (the ring's nanoseconds and
//! the kernel's milliseconds meet in one line), so the stamp a cycle records can differ from the one
//! the policy host reported by a nanosecond. [`STAMP_TOLERANCE_NS`] is a microsecond: three orders
//! of magnitude wider than that error, and three narrower than the period between two commands on
//! any body this OS drives, so a command can never fall within tolerance of another session's.
//!
//! # A session with no end
//!
//! A policy host that stopped without saying `closed` (it crashed, or the ring was sealed while the
//! session was open) leaves a session with a first proposal and no last. It ends where the next
//! session's first proposal begins, or at the end of the recording — and since nothing else
//! commands while a remote policy is installed, that bound is the true one.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::record::{Body, Cycle, EventCode, Record, EVENT_DETAIL, EVENT_SUBJECT};

/// How far a cycle's recorded command stamp may be from the one the policy host reported.
pub const STAMP_TOLERANCE_NS: u64 = 1_000;

/// One command, as the policy host that published it identifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandMark {
    /// The command ring's sequence number (§25). For a person reading the recording.
    pub seq: u64,
    /// When the policy produced it, on the kernel's clock (§24). The join key.
    pub stamp_ns: u64,
}

/// What one [`EventCode::RemoteSession`] detail says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// The session was established, serving this model.
    Opened {
        /// The model digest the session serves, as the replica reported it.
        digest: String,
    },
    /// The session's first proposal reached the command ring.
    Proposing(CommandMark),
    /// The session ended; this was its last proposal.
    Closed(CommandMark),
}

/// Why a session's facts cannot be written as an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteRecordError {
    /// The session id is empty, too long for the subject, or not printable ASCII.
    Session,
    /// The digest is not `sha256:` and sixty-four lowercase hex digits, or bare hex of that length.
    Digest,
}

/// Whether `session` can be an event's subject: printable ASCII without spaces, at most the
/// subject's width. A session id the Hub issues is a UUID; anything else is refused rather than
/// truncated, because a truncated id would name a different session.
pub fn session_subject(session: &str) -> Result<&str, RemoteRecordError> {
    if session.is_empty()
        || session.len() > EVENT_SUBJECT
        || !session.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(RemoteRecordError::Session);
    }
    Ok(session)
}

/// A model digest in the one spelling the recording keeps: `sha256:` and sixty-four lowercase hex
/// digits. Bare hex is accepted and prefixed; anything else is refused.
pub fn digest_spelling(digest: &str) -> Result<String, RemoteRecordError> {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RemoteRecordError::Digest);
    }
    Ok(format!("sha256:{}", hex.to_ascii_lowercase()))
}

/// The detail sentence for what was said. Always fits [`EVENT_DETAIL`]: the longest is 78 bytes.
pub fn detail(said: &Said) -> String {
    let text = match said {
        Said::Opened { digest } => format!("opened {digest}"),
        Said::Proposing(mark) => format!(
            "proposing from command {} stamped {}",
            mark.seq, mark.stamp_ns
        ),
        Said::Closed(mark) => format!(
            "closed through command {} stamped {}",
            mark.seq, mark.stamp_ns
        ),
    };
    debug_assert!(text.len() <= EVENT_DETAIL);
    text
}

/// Read a detail back. `None` for anything that is not one of the three sentences, which a reader
/// reports as unreadable rather than guessing at.
pub fn parse_detail(detail: &str) -> Option<Said> {
    if let Some(digest) = detail.strip_prefix("opened ") {
        return digest_spelling(digest)
            .ok()
            .map(|digest| Said::Opened { digest });
    }
    let mark = |rest: &str| -> Option<CommandMark> {
        let (seq, stamp) = rest.split_once(" stamped ")?;
        Some(CommandMark {
            seq: seq.parse().ok()?,
            stamp_ns: stamp.parse().ok()?,
        })
    };
    if let Some(rest) = detail.strip_prefix("proposing from command ") {
        return mark(rest).map(Said::Proposing);
    }
    if let Some(rest) = detail.strip_prefix("closed through command ") {
        return mark(rest).map(Said::Closed);
    }
    None
}

/// One remote session, as the recording knows it.
///
/// Every field but the id may be missing: the ring is a window, and the event that said a thing
/// may have been overwritten while a later one survived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSession {
    /// The session id the Hub issued.
    pub session: String,
    /// The model it served, when the `opened` event is still in the ring.
    pub digest: Option<String>,
    /// Its first proposal, when the `proposing` event is still in the ring.
    pub first: Option<CommandMark>,
    /// Its last proposal, when it ended and said so.
    pub last: Option<CommandMark>,
}

/// Every remote session the records name, in the order their first proposals were stamped; sessions
/// that never proposed come last, in the order they were opened. Details that are not one of the
/// three sentences are counted in the second value rather than guessed at.
pub fn sessions(records: &[Record]) -> (Vec<RemoteSession>, u64) {
    let mut out: Vec<RemoteSession> = Vec::new();
    let mut unreadable = 0u64;
    for record in records {
        let Body::Event(event) = &record.body else {
            continue;
        };
        if event.code != EventCode::RemoteSession {
            continue;
        }
        let Some(said) = parse_detail(&event.detail) else {
            unreadable += 1;
            continue;
        };
        let index = match out.iter().position(|entry| entry.session == event.subject) {
            Some(index) => index,
            None => {
                out.push(RemoteSession {
                    session: event.subject.to_string(),
                    digest: None,
                    first: None,
                    last: None,
                });
                out.len() - 1
            }
        };
        let entry = &mut out[index];
        match said {
            Said::Opened { digest } => entry.digest = Some(digest),
            Said::Proposing(mark) => entry.first = Some(mark),
            Said::Closed(mark) => entry.last = Some(mark),
        }
    }
    // Stable, so the never-proposed keep their opening order behind the ones that proposed.
    out.sort_by_key(|entry| entry.first.map_or(u64::MAX, |mark| mark.stamp_ns));
    (out, unreadable)
}

/// The session whose proposal this cycle executed, if any.
///
/// `sessions` in the order [`sessions`] returns them. A cycle that executed no command, or a
/// command that commanded no channel (a policy host's own hold), belongs to none.
pub fn attribute<'a>(sessions: &'a [RemoteSession], cycle: &Cycle) -> Option<&'a RemoteSession> {
    if cycle.command_seq == 0 || !cycle.desired.iter().any(|value| value.is_finite()) {
        return None;
    }
    let stamp = cycle.command_stamp_ns;
    let proposed: Vec<&RemoteSession> = sessions
        .iter()
        .filter(|entry| entry.first.is_some())
        .collect();
    for (index, entry) in proposed.iter().enumerate() {
        let first = entry.first.map_or(0, |mark| mark.stamp_ns);
        let low = first.saturating_sub(STAMP_TOLERANCE_NS);
        let high = match (entry.last, proposed.get(index + 1)) {
            (Some(last), _) => last.stamp_ns.saturating_add(STAMP_TOLERANCE_NS),
            // No end said: the next session's first proposal is the end, exclusive.
            (None, Some(next)) => next.first.map_or(u64::MAX, |mark| {
                mark.stamp_ns.saturating_sub(STAMP_TOLERANCE_NS + 1)
            }),
            (None, None) => u64::MAX,
        };
        if (low..=high).contains(&stamp) {
            return Some(entry);
        }
    }
    None
}

/// How many of the records' cycles ran on each session's proposals, and how many on none.
pub fn cycles_by_session(records: &[Record], sessions: &[RemoteSession]) -> (Vec<u64>, u64) {
    let mut counts = alloc::vec![0u64; sessions.len()];
    let mut local = 0u64;
    for record in records {
        let Body::Cycle(cycle) = &record.body else {
            continue;
        };
        match attribute(sessions, cycle) {
            Some(entry) => {
                let index = sessions
                    .iter()
                    .position(|candidate| core::ptr::eq(candidate, entry))
                    .unwrap_or(0);
                counts[index] += 1;
            }
            None => local += 1,
        }
    }
    (counts, local)
}
