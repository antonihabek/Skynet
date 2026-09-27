//! A remote policy's sessions and counts, carried out of its policy host's queue (§90.2, §74).
//!
//! `skynetp --backend remote` holds the session and is the only process that knows which of its
//! commands came from which session and model, and how often its deadline fired. It is also the
//! process that talks to the network, so it holds no descriptor of the black box: it writes each
//! fact as one small file into a queue beside its bridge socket (`skynet_policy_host::provenance`),
//! and this module carries the facts across on every reconcile pass — the arrangement
//! [`crate::blackbox::mirror_audit`] already uses for a configuration change another process made.
//!
//! * **Session facts** become §26 `remote_session` events, in `skynet_blackbox::remote`'s grammar,
//!   so a reader can join each cycle to the session whose proposal it executed.
//! * **Counts** are added to the telemetry window open when they arrive (§74's `remote_proposals`
//!   and `remote_stale`).
//!
//! A file is deleted once it has been carried, and left in place when it could not be — a session
//! fact on a robot whose black box would not open waits for one that does, rather than being lost.
//!
//! # A run that did not say it ended
//!
//! Counts arrive as deltas, one record in flight at a time, and a run's last record is marked
//! `final`. A policy host stopped by a signal writes no final record, and whatever it counted since
//! its last record is gone — so a window in which one run gives way to another without a final
//! record would carry a **floor**, which §74.6 forbids presenting as a count. That window reports
//! the two counters as unmeasured, and the next one counts normally.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skynet_blackbox::record::{EventCode, Gap, GapReason};
use skynet_blackbox::remote::{detail, digest_spelling, session_subject, CommandMark, Said};
use skynet_json::Json;

use crate::layout::Layout;

/// Records carried per pass, across every queue. A bound for §33.2's loop, not a rate anything
/// reaches: a policy host writes a session fact a few times a minute at most.
pub const DRAIN_MAX_PER_PASS: usize = 256;

/// The suffix of a package's queue directory beside its bridge socket.
pub const QUEUE_SUFFIX: &str = ".provenance";

/// What a remote policy counted, over the part of a window that has arrived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteCounting {
    /// Commands it proposed (§74's `remote_proposals`).
    pub proposals: u64,
    /// Turns it stayed quiet because its newest chunk was older than its deadline (`remote_stale`).
    pub stale: u64,
    /// A run gave way to another without saying it ended, inside this window: the counts are a
    /// floor and must not be reported as a count.
    pub incomplete: bool,
}

impl RemoteCounting {
    /// Fold another share of the window in.
    pub fn add(&mut self, other: RemoteCounting) {
        self.proposals = self.proposals.saturating_add(other.proposals);
        self.stale = self.stale.saturating_add(other.stale);
        self.incomplete |= other.incomplete;
    }
}

/// Which run each queue last heard from, and whether it said it had ended. Kept across windows by
/// the daemon, in memory: a daemon that restarts starts with nothing, which reads the first run it
/// then hears from as a continuation rather than inventing a crash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Continuity {
    runs: BTreeMap<String, (String, bool)>,
}

/// One record out of a queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    /// A session fact, with how many facts before it the policy host could not queue.
    Fact {
        /// The run that wrote it.
        run: String,
        /// The session.
        session: String,
        /// What it says.
        said: Said,
        /// Facts this run could not queue before this one.
        unqueued_before: u64,
    },
    /// Counts since the run's previous counts record.
    Counts {
        /// The run that wrote it.
        run: String,
        /// Proposals.
        proposals: u64,
        /// Turns quiet for a stale chunk.
        stale: u64,
        /// The run's last record.
        last: bool,
    },
}

/// Decode one queue file, strictly: a record that is not exactly one of the four shapes is refused
/// with the reason, rather than read charitably.
pub fn decode(text: &str) -> Result<Record, String> {
    let json = Json::parse(text).map_err(|error| format!("not JSON: {error:?}"))?;
    let text_of = |key: &str| -> Result<String, String> {
        json.get(key)
            .and_then(Json::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("`{key}` is missing or not a string"))
    };
    let number_of = |key: &str| -> Result<u64, String> {
        text_of(key)?
            .parse::<u64>()
            .map_err(|_| format!("`{key}` is not a whole number"))
    };
    let run = text_of("run")?;
    let kind = text_of("kind")?;
    if kind == "counts" {
        return Ok(Record::Counts {
            run,
            proposals: number_of("proposals")?,
            stale: number_of("stale")?,
            last: json
                .get("final")
                .and_then(Json::as_bool)
                .ok_or("`final` is missing or not a boolean")?,
        });
    }
    let session = text_of("session")?;
    session_subject(&session).map_err(|_| "`session` cannot be an event subject".to_string())?;
    let mark = || -> Result<CommandMark, String> {
        Ok(CommandMark {
            seq: number_of("seq")?,
            stamp_ns: number_of("stamp_ns")?,
        })
    };
    let said = match kind.as_str() {
        "opened" => Said::Opened {
            digest: digest_spelling(&text_of("digest")?)
                .map_err(|_| "`digest` is not a sha256 digest".to_string())?,
        },
        "proposing" => Said::Proposing(mark()?),
        "closed" => Said::Closed(mark()?),
        other => return Err(format!("`kind` {other:?} is not one this daemon carries")),
    };
    let unqueued_before = match json.get("unqueued_before") {
        None => 0,
        Some(_) => number_of("unqueued_before")?,
    };
    Ok(Record::Fact {
        run,
        session,
        said,
        unqueued_before,
    })
}

/// What one pass carried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drained {
    /// Session facts written into the ring.
    pub events: usize,
    /// Counts that arrived, when any did.
    pub counts: Option<RemoteCounting>,
    /// Files that could not be read, recorded as such and removed.
    pub unreadable: usize,
    /// Files left for the next pass.
    pub left: usize,
}

/// Every queue directory beside the bridge sockets, in name order.
pub fn queues(layout: &Layout) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(layout.remote_run()) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(QUEUE_SUFFIX))
        })
        .collect();
    out.sort();
    out
}

fn pending(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".json") && !name.starts_with('.'))
        })
        .collect();
    out.sort();
    out
}

/// Carry every queue's records: session facts into `recorder`, counts into the returned tally.
///
/// With no recorder, counts are still carried — the window they belong to exists whether or not
/// the black box does — and session facts wait for a ring to be written into.
pub fn drain(
    layout: &Layout,
    mut recorder: Option<&mut crate::blackbox::Recorder>,
    mono_ns: u64,
    continuity: &mut Continuity,
) -> Drained {
    let mut out = Drained::default();
    let mut budget = DRAIN_MAX_PER_PASS;
    for queue in queues(layout) {
        let key = queue
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        for path in pending(&queue) {
            if budget == 0 {
                out.left += 1;
                continue;
            }
            budget -= 1;
            let decoded = std::fs::read_to_string(&path)
                .map_err(|error| format!("unreadable: {error}"))
                .and_then(|text| decode(&text));
            match decoded {
                Ok(Record::Counts {
                    run,
                    proposals,
                    stale,
                    last,
                }) => {
                    let mut share = RemoteCounting {
                        proposals,
                        stale,
                        incomplete: false,
                    };
                    if let Some((previous, ended)) = continuity.runs.get(&key) {
                        if *previous != run && !*ended {
                            share.incomplete = true;
                        }
                    }
                    continuity.runs.insert(key.clone(), (run, last));
                    out.counts
                        .get_or_insert_with(RemoteCounting::default)
                        .add(share);
                    let _ = std::fs::remove_file(&path);
                }
                Ok(Record::Fact {
                    session,
                    said,
                    unqueued_before,
                    ..
                }) => {
                    let Some(ring) = recorder.as_deref_mut() else {
                        out.left += 1;
                        continue;
                    };
                    if unqueued_before > 0 {
                        // §47.4: facts the policy host could not queue are records that should
                        // exist and do not, and the recording says so rather than looking whole.
                        let gap = Gap {
                            dropped: unqueued_before,
                            first_tick: 0,
                            reason: GapReason::WriterBehind,
                        };
                        if ring.gap(mono_ns, gap).is_err() {
                            out.left += 1;
                            continue;
                        }
                    }
                    if ring
                        .event(mono_ns, EventCode::RemoteSession, &session, &detail(&said))
                        .is_err()
                    {
                        // Left in place: the next pass writes it, and nothing after it in this
                        // queue is carried ahead of it.
                        out.left += 1;
                        break;
                    }
                    out.events += 1;
                    let _ = std::fs::remove_file(&path);
                }
                Err(why) => {
                    // Recorded as an unreadable `remote_session`, which the join counts rather than
                    // guesses at, and removed: a file that will never decode must not block the
                    // queue behind it.
                    if let Some(ring) = recorder.as_deref_mut() {
                        let name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default();
                        let _ = ring.event(
                            mono_ns,
                            EventCode::RemoteSession,
                            "unreadable",
                            &format!("{name}: {why}"),
                        );
                    }
                    out.unreadable += 1;
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    out
}
