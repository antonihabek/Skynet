//! What a remote policy's session did, told to the supervisor (§90.2, §74).
//!
//! Two facts leave this process about a remote policy, and neither is a command:
//!
//! * **Which session proposed which commands** — §90.2's *"the recording notes which cycles ran on
//!   remote proposals, with the session id and the model digest."* Only this process knows: the
//!   session and the digest arrive over the bridge, and the command ring (§25) carries no provenance.
//! * **How often the deadline fired** — the turns this policy stayed quiet because the newest chunk
//!   it held was older than `max_chunk_age_ms`, beside the turns it proposed. §74's window is where
//!   a fleet sees that, and before this module it reached only this process's end-of-run summary.
//!
//! # Through a queue, never into the recording
//!
//! This is the process that talks to the network, and it holds no descriptor of the black box — the
//! same line §33.1 draws around the policy sandbox. So it writes each fact as one small JSON file
//! into a directory the supervisor gives it, and `skynetd` carries each into the ring (as a
//! `remote_session` event) or into the open telemetry window (as counts), then deletes it. A file
//! is written whole and renamed into place, so the supervisor never reads half of one.
//!
//! # Counts are deltas, and one is in flight at a time
//!
//! A running total would make the supervisor subtract two readings of a process that may have
//! restarted in between. So each counts record carries what happened since the previous one, and a
//! new one is written only once the supervisor has taken the last — the deltas accumulate here in
//! the meantime, so nothing is lost while the supervisor is slow and nothing piles up while it is
//! stopped. The last record of a run says `final`, which is how the supervisor tells a run that
//! ended from one that crashed with counts it never wrote.
//!
//! # Numbers that must stay exact are strings
//!
//! Stamps are nanoseconds since boot; after about a hundred days they pass the 2⁵³ an `f64` holds
//! exactly, and the JSON here has only one number type. The join (`skynet_blackbox::remote`) is on
//! those stamps, so they travel as decimal strings.

use std::path::{Path, PathBuf};

use skynet_json::Json;

/// One published command, as the join identifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The command ring's sequence number (§25).
    pub seq: u64,
    /// When the policy produced it, on the kernel's clock (§24).
    pub stamp_ns: u64,
}

/// One fact about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    /// A session was established, serving this model.
    Opened {
        /// The session id the Hub issued.
        session: String,
        /// The model digest the replica reported.
        digest: String,
    },
    /// Its first proposal reached the ring.
    Proposing {
        /// The session.
        session: String,
        /// The command.
        mark: Mark,
    },
    /// It ended, and this was its last proposal. Not said for a session that never proposed.
    Closed {
        /// The session.
        session: String,
        /// The command.
        mark: Mark,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    session: String,
    first: Option<Mark>,
    last: Option<Mark>,
}

/// The pure half: what each turn changed about the session, as facts. No file, no clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tracker {
    open: Option<Open>,
}

impl Tracker {
    /// One turn: the session the backend holds after it (`(id, digest)`), and the command it
    /// proposed during it, if one reached the ring. A policy host's own hold is not a proposal and
    /// is not passed here.
    pub fn observe(&mut self, session: Option<(&str, &str)>, proposed: Option<Mark>) -> Vec<Fact> {
        let mut facts = Vec::new();
        let same =
            matches!((&self.open, session), (Some(open), Some((id, _))) if open.session == id);
        if !same {
            if let Some(ended) = self.open.take() {
                if let Some(mark) = ended.last {
                    facts.push(Fact::Closed {
                        session: ended.session,
                        mark,
                    });
                }
            }
            if let Some((id, digest)) = session {
                facts.push(Fact::Opened {
                    session: id.to_string(),
                    digest: digest.to_string(),
                });
                self.open = Some(Open {
                    session: id.to_string(),
                    first: None,
                    last: None,
                });
            }
        }
        if let (Some(open), Some(mark)) = (self.open.as_mut(), proposed) {
            if open.first.is_none() {
                open.first = Some(mark);
                facts.push(Fact::Proposing {
                    session: open.session.clone(),
                    mark,
                });
            }
            open.last = Some(mark);
        }
        facts
    }

    /// The process is stopping: the open session, if it proposed, is closed where it stands.
    pub fn finish(&mut self) -> Vec<Fact> {
        self.observe(None, None)
    }
}

/// A run's name: when it started on the kernel's clock, then the process id. The queue lives in
/// `/run`, which a reboot empties, so the clock never restarts under names already in it.
pub fn run_name(started_ns: u64, pid: u32) -> String {
    format!("{started_ns:020}-{pid}")
}

/// How many files the queue may hold before a session fact is not queued. Facts are written on a
/// session's changes, a few a minute at worst; this is a bound for a supervisor that has stopped
/// reading, not a rate anything reaches.
pub const QUEUE_MAX: usize = 1024;

/// The fs half: facts and counts into the supervisor's queue.
#[derive(Debug)]
pub struct Provenance {
    dir: PathBuf,
    run: String,
    written: u64,
    tracker: Tracker,
    // What the backend had counted at the last counts record written, and what has accumulated
    // since while the previous record was still in flight.
    reported: (u64, u64),
    in_flight: Option<PathBuf>,
    unqueued: u64,
}

impl Provenance {
    /// Write into `dir`, naming this run `run` — something no other run of this package shares, so
    /// the supervisor can tell a restart from a continuation, and that sorts in the order runs
    /// started ([`run_name`]), so it carries one run's records before the next one's.
    pub fn open(dir: &Path, run: String) -> Provenance {
        let _ = std::fs::create_dir_all(dir);
        Provenance {
            dir: dir.to_path_buf(),
            run,
            written: 0,
            tracker: Tracker::default(),
            reported: (0, 0),
            in_flight: None,
            unqueued: 0,
        }
    }

    /// One turn. `proposals` and `stale` are the backend's running counts (`commanded`,
    /// `quiet_stale`); the delta since the last record is what is written.
    pub fn turn(
        &mut self,
        session: Option<(&str, &str)>,
        proposed: Option<Mark>,
        proposals: u64,
        stale: u64,
    ) {
        for fact in self.tracker.observe(session, proposed) {
            self.write_fact(&fact);
        }
        self.write_counts(proposals, stale, false);
    }

    /// The run is ending: close the open session and write the last counts, marked final.
    pub fn finish(&mut self, proposals: u64, stale: u64) {
        for fact in self.tracker.finish() {
            self.write_fact(&fact);
        }
        self.write_counts(proposals, stale, true);
    }

    /// Facts not queued because the queue was full, since the last one that was.
    pub fn unqueued(&self) -> u64 {
        self.unqueued
    }

    fn write_fact(&mut self, fact: &Fact) {
        let pending = std::fs::read_dir(&self.dir)
            .map(|entries| entries.count())
            .unwrap_or(0);
        if pending >= QUEUE_MAX {
            self.unqueued += 1;
            return;
        }
        let mut record = Json::obj();
        let (kind, session) = match fact {
            Fact::Opened { session, digest } => {
                record.set("digest", Json::Str(digest.clone()));
                ("opened", session)
            }
            Fact::Proposing { session, mark } | Fact::Closed { session, mark } => {
                record.set("seq", Json::Str(mark.seq.to_string()));
                record.set("stamp_ns", Json::Str(mark.stamp_ns.to_string()));
                (
                    if matches!(fact, Fact::Proposing { .. }) {
                        "proposing"
                    } else {
                        "closed"
                    },
                    session,
                )
            }
        };
        record.set("kind", Json::Str(kind.to_string()));
        record.set("session", Json::Str(session.clone()));
        // The facts this run could not queue before this one, said once, on the next that was:
        // the supervisor records them as a gap rather than a recording that looks whole.
        if self.unqueued > 0 {
            record.set("unqueued_before", Json::Str(self.unqueued.to_string()));
        }
        if self.put(&mut record).is_some() {
            self.unqueued = 0;
        }
    }

    fn write_counts(&mut self, proposals: u64, stale: u64, last: bool) {
        // One in flight: the supervisor deletes a record once it has taken it.
        if let Some(path) = &self.in_flight {
            if path.exists() && !last {
                return;
            }
        }
        let delta = (
            proposals.saturating_sub(self.reported.0),
            stale.saturating_sub(self.reported.1),
        );
        let mut record = Json::obj();
        record.set("kind", Json::Str("counts".to_string()));
        record.set("proposals", Json::Str(delta.0.to_string()));
        record.set("stale", Json::Str(delta.1.to_string()));
        record.set("final", Json::Bool(last));
        if let Some(path) = self.put(&mut record) {
            self.reported = (proposals, stale);
            self.in_flight = Some(path);
        }
    }

    /// Write one record whole, then rename it into place. `None` when it could not be written,
    /// which leaves the counts to be carried in the next one.
    fn put(&mut self, record: &mut Json) -> Option<PathBuf> {
        record.set("run", Json::Str(self.run.clone()));
        let text = record.to_canonical().ok()?;
        self.written += 1;
        // Ordered by run and then by record, so the supervisor carries one run's facts in order.
        let name = format!("{}-{:010}.json", self.run, self.written);
        let path = self.dir.join(&name);
        let partial = self.dir.join(format!(".{name}.partial"));
        std::fs::write(&partial, text.as_bytes()).ok()?;
        std::fs::rename(&partial, &path).ok()?;
        Some(path)
    }
}
