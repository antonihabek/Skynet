//! The black box, against a disk (§13, §28, §33.2).
//!
//! [`skynet_blackbox`] is the format and the arithmetic and touches nothing; this is the half that
//! opens files. It preallocates the ring at `<root>/data/blackbox/ring`, writes one record per
//! positioned write, and seals into `<root>/data/blackbox/sealed/`.
//!
//! # Preallocation is a real write, not a hole
//!
//! The ring is created by writing its whole length, zeroes included. A sparse file would report the
//! same size and fail at the first write after the disk filled, which is precisely the failure §28
//! spends a paragraph on: *a black box that stops recording because a log filled the disk is a
//! black box that fails exactly when something is going wrong.* The bytes are claimed while there
//! are bytes to claim.
//!
//! # What this recorder can currently record, and what it cannot
//!
//! Events, gaps, and nothing else — and that is a statement about the rest of the system rather
//! than about this module. §26's other five channels carry control cycles, and there is no control
//! loop yet: the policy runtime, the SPSC ring and observation assembly (§25) are unwritten. So the
//! ring this daemon opens has a shape of **zero joints and zero channels**, which is the honest
//! description of a robot that is supervising packages and not commanding a body.
//!
//! Two consequences are deliberate:
//!
//! * A sealed file from this edition has an empty `/sk/state`, `/sk/desired`, `/sk/verdict` and
//!   `/sk/command`, and [`Recorder::index`] says so in as many words. An incident report that
//!   looked complete because nobody had noticed the cycles were missing would be worse than no
//!   incident report.
//! * The ring is **recreated** when the shape changes, because a record's size is a function of the
//!   shape and a ring cannot hold two sizes. The recording that was there is sealed first, so
//!   turning on a control loop costs the previous window a file rather than costing it existence.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use skynet_blackbox::record::{Entry, EventCode, EventView, Gap, Shape};
use skynet_blackbox::ring::{Geometry, Meta, Ring};
use skynet_blackbox::seal::{seal, Reason, Sealed};
use skynet_json::Json;

use crate::layout::Layout;

/// How many bytes the ring is preallocated with, absent anything that says otherwise.
///
/// Sixty-four mebibytes, which is a number and not a duration on purpose. §13 asks for "roughly an
/// hour", and what an hour costs depends entirely on the body: at this size a 30-joint humanoid
/// logging at 1 kHz gets about seventy seconds, and a supervisor logging only events gets years.
/// [`Recorder::index`] reports the window this actually buys rather than repeating the aspiration.
pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;

/// How many distinct policy digests the header table holds.
pub const DIGESTS: u16 = 16;

/// How many distinct capability tokens it holds.
pub const CAPABILITIES: u16 = 16;

/// The recorder, and the file under it.
#[derive(Debug)]
pub struct Recorder {
    ring: Ring,
    path: PathBuf,
    sealed: PathBuf,
    scratch: Vec<u8>,
}

fn io_error(doing: &str, path: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{doing} {}: {error}", path.display()))
}

impl Recorder {
    /// Open the ring, creating and preallocating it if it is absent or the wrong shape.
    ///
    /// A ring whose header does not open — a damaged CRC, an unknown format version, a shape that
    /// no longer matches the body — is **sealed if it can be read at all, then replaced**. The
    /// alternative, refusing to start, would leave a robot with no recorder because of a recording,
    /// which inverts what the subsystem is for.
    pub fn open(
        layout: &Layout,
        meta: Meta,
        shape: Shape,
        budget: usize,
    ) -> std::io::Result<Recorder> {
        let dir = layout.blackbox();
        let sealed = dir.join("sealed");
        fs::create_dir_all(&sealed).map_err(|e| io_error("creating", &sealed, e))?;
        let path = dir.join("ring");

        if let Ok(store) = fs::read(&path) {
            if let Ok(ring) = Ring::open(&store) {
                if ring.geometry().shape == shape {
                    let scratch = vec![0u8; ring.geometry().record_size];
                    return Ok(Recorder {
                        ring,
                        path,
                        sealed,
                        scratch,
                    });
                }
                // The body changed under the recording. Keep what is there as a file before the
                // ring is replaced: the records are still true, they are just about a different
                // shape of robot.
                let closing = seal(&ring, &store, Reason::Shutdown, 0);
                let _ = write_sealed(&sealed, &closing, 0);
            }
        }

        let geometry = Geometry::for_budget(shape, DIGESTS, CAPABILITIES, budget)
            .map_err(|error| std::io::Error::other(format!("sizing the black box: {error:?}")))?;
        let (ring, store) = Ring::create(geometry, meta);
        let mut file = File::create(&path).map_err(|e| io_error("creating", &path, e))?;
        file.write_all(&store)
            .map_err(|e| io_error("preallocating", &path, e))?;
        file.sync_all()
            .map_err(|e| io_error("flushing", &path, e))?;
        let scratch = vec![0u8; ring.geometry().record_size];
        Ok(Recorder {
            ring,
            path,
            sealed,
            scratch,
        })
    }

    /// The geometry this recording has.
    pub fn geometry(&self) -> Geometry {
        self.ring.geometry()
    }

    /// Record something that changed.
    pub fn event(
        &mut self,
        mono_ns: u64,
        code: EventCode,
        subject: &str,
        detail: &str,
    ) -> std::io::Result<u64> {
        let view = EventView {
            code,
            subject,
            detail,
        };
        self.write(mono_ns, Entry::Event(&view))
    }

    /// Record that records are missing (§47.4).
    pub fn gap(&mut self, mono_ns: u64, gap: Gap) -> std::io::Result<u64> {
        self.write(mono_ns, Entry::Gap(gap))
    }

    fn write(&mut self, mono_ns: u64, entry: Entry<'_>) -> std::io::Result<u64> {
        let (sequence, offset) = self
            .ring
            .stage(&mut self.scratch, mono_ns, entry)
            .map_err(|error| std::io::Error::other(format!("encoding a record: {error:?}")))?;
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|e| io_error("opening", &self.path, e))?;
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|e| io_error("seeking in", &self.path, e))?;
        file.write_all(&self.scratch)
            .map_err(|e| io_error("writing to", &self.path, e))?;
        Ok(sequence)
    }

    /// Read the ring back.
    ///
    /// The whole file, because sealing produces the whole file. A ring sized in gigabytes would want
    /// a streaming reader; the answer to that is a smaller ring, since §34 already says a sealed
    /// recording is uploaded and a gigabyte of it is not going anywhere over a metered bearer.
    fn read(&self) -> std::io::Result<Vec<u8>> {
        let mut file = File::open(&self.path).map_err(|e| io_error("opening", &self.path, e))?;
        let mut store = Vec::new();
        file.read_to_end(&mut store)
            .map_err(|e| io_error("reading", &self.path, e))?;
        Ok(store)
    }

    /// Seal the window into a file, and return what was written and where.
    pub fn seal(&self, reason: Reason, mono_ns: u64) -> std::io::Result<(Sealed, PathBuf)> {
        let store = self.read()?;
        let sealed = seal(&self.ring, &store, reason, mono_ns);
        let path = write_sealed(&self.sealed, &sealed, mono_ns)?;
        Ok((sealed, path))
    }

    /// Sealed recordings on disk, newest name last.
    pub fn sealed(&self) -> Vec<(String, u64)> {
        let mut found: Vec<(String, u64)> = fs::read_dir(&self.sealed)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".mcap") {
                    return None;
                }
                let size = entry.metadata().ok()?.len();
                Some((name, size))
            })
            .collect();
        found.sort();
        found
    }

    /// One sealed recording's bytes, by name.
    ///
    /// The name is checked rather than joined: a caller-supplied path segment is how a download
    /// route becomes a way to read `/etc/shadow`. Anything but a plain `.mcap` file name is refused.
    pub fn read_sealed(&self, name: &str) -> Option<Vec<u8>> {
        if !name.ends_with(".mcap")
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
        {
            return None;
        }
        fs::read(self.sealed.join(name)).ok()
    }

    /// What the console and `skynet blackbox` show: the window, and what is not in it.
    pub fn index(&self, rate_hz: f64) -> Json {
        let store = self.read().unwrap_or_default();
        let sealed = seal(&self.ring, &store, Reason::Operator, 0);
        let geometry = self.ring.geometry();

        let mut document = Json::obj();
        document.set(
            "format_version",
            Json::Num(skynet_blackbox::FORMAT_VERSION as f64),
        );
        document.set("record_size", Json::Num(geometry.record_size as f64));
        document.set("capacity", Json::Num(geometry.capacity as f64));
        document.set("bytes", Json::Num(geometry.total_bytes() as f64));
        document.set(
            "seconds_covered_at_rate",
            Json::Num(geometry.seconds_covered(rate_hz)),
        );
        document.set("rate_hz", Json::Num(rate_hz));
        document.set("joints", Json::Num(geometry.shape.joints as f64));
        document.set("channels", Json::Num(geometry.shape.channels as f64));
        document.set("next_sequence", Json::Num(self.ring.next_sequence() as f64));

        let mut window = Json::obj();
        window.set("records", Json::Num(sealed.continuity.records as f64));
        window.set("first_sequence", Json::Num(sealed.continuity.first as f64));
        window.set("last_sequence", Json::Num(sealed.continuity.last as f64));
        window.set("cycles", Json::Num(sealed.continuity.cycles as f64));
        window.set(
            "interventions",
            Json::Num(sealed.continuity.interventions as f64),
        );
        window.set("damaged", Json::Num(sealed.continuity.damaged as f64));
        window.set("dropped", Json::Num(sealed.continuity.dropped as f64));
        let mut holes = Json::arr();
        for hole in &sealed.continuity.holes {
            let mut entry = Json::obj();
            entry.set("from", Json::Num(hole.from as f64));
            entry.set("to", Json::Num(hole.to as f64));
            holes.push(entry);
        }
        window.set("holes", holes);
        window.set("complete", Json::Bool(sealed.continuity.complete()));
        document.set("window", window);

        // The sentence that keeps this honest. §26 gives the black box six channels and this
        // edition can fill one of them, because the thing that produces the other five does not
        // exist. A console that showed a healthy-looking recording without saying so would be
        // reporting a complete black box on a robot that has never recorded a control cycle.
        if geometry.shape.channels == 0 {
            document.set(
                "cycles_unavailable",
                Json::Str(
                    "This recording holds events only. §26's state, desired, verdict and command \
                     channels carry control cycles, and there is no control loop on this robot: \
                     the policy runtime and observation assembly (§25) are unwritten."
                        .into(),
                ),
            );
        }

        let mut files = Json::arr();
        for (name, size) in self.sealed() {
            let mut entry = Json::obj();
            entry.set("name", Json::Str(name));
            entry.set("bytes", Json::Num(size as f64));
            files.push(entry);
        }
        document.set("sealed", files);
        document
    }
}

/// Write a sealed recording, named for when and why.
///
/// The monotonic instant rather than a wall clock: §35.1 gives `CLOCK_REALTIME` one job and naming
/// files is not it, and a robot whose clock was set between two incidents would otherwise produce
/// two files whose names imply the wrong order.
/// What a seal produced, as an incident names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    /// The file name under `<blackbox>/sealed/`.
    pub name: String,
    /// How many bytes it is.
    pub size_bytes: u64,
    /// `sha256:…` over those bytes — §56.4's black box segment hash.
    pub digest: String,
    /// How many records the window holds.
    pub records: u64,
    /// Records the ring could not read back, which are holes rather than absences.
    pub damaged: u64,
}

impl Recording {
    /// The shape an incident carries.
    ///
    /// **One shape, with a discriminant**, rather than an object when a seal worked and a sentence
    /// when it did not. A union would make every consumer switch on a JSON type before it could
    /// read a field, and the first one that forgot would render `[object Object]` or crash on a
    /// robot with no ring — which is the ordinary case on this edition. See [`unsealed`].
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("sealed", Json::Bool(true))
            .set("name", Json::Str(self.name.clone()))
            .set("size_bytes", Json::Num(self.size_bytes as f64))
            .set("digest", Json::Str(self.digest.clone()))
            .set("records", Json::Num(self.records as f64))
            .set("damaged", Json::Num(self.damaged as f64))
            // The one field a Hub needs and cannot infer. §13 calls this an upload and §17 takes a
            // document; nothing carries the bytes, so the incident says where they are rather than
            // leaving a reader to assume they are attached.
            .set("uploaded", Json::Bool(false));
        out
    }
}

/// The same shape, for an incident whose window could not be sealed.
///
/// A reason rather than an omission, for §7.2's rule: a consumer that sees no `recording` key
/// cannot tell a robot with no black box from an edition that never attached one, and those send an
/// investigator to two different places.
pub fn unsealed(why: impl Into<String>) -> Json {
    let mut out = Json::obj();
    out.set("sealed", Json::Bool(false))
        .set("why", Json::Str(why.into()));
    out
}

/// Seal whatever ring is on the disk, without opening one for writing (§13).
///
/// **It reads; it never creates.** [`Recorder::open`] deliberately replaces a ring whose shape no
/// longer matches the body, which is right for the one process that owns the recording and
/// catastrophic for anybody else — a second opener on a robot whose descriptor had changed would
/// wipe the window somebody is about to seal. So this takes no `Recorder`, holds no handle, and can
/// be called from anywhere that has just decided something worth keeping happened.
///
/// `Ok(None)` where there is no ring, which is not a failure: §28 preallocates one on a robot that
/// has a recorder, and a robot without one — `skynetd once`, a bare development root — has nothing
/// to seal. The incident then says so, which is a named absence rather than a missing field.
pub fn seal_existing(
    layout: &Layout,
    reason: Reason,
    mono_ns: u64,
) -> std::io::Result<Option<Recording>> {
    let dir = layout.blackbox();
    let path = dir.join("ring");
    let Ok(store) = fs::read(&path) else {
        return Ok(None);
    };
    let Ok(ring) = Ring::open(&store) else {
        // A ring whose header does not open is a damaged recording, not a missing one, and the
        // difference matters to whoever reads the incident: there was a black box and it is not
        // readable, which is itself a fact about the machine.
        return Err(std::io::Error::other(format!(
            "{} does not open as a ring: the window this incident would have carried is not \
             readable",
            path.display()
        )));
    };

    let sealed_dir = dir.join("sealed");
    fs::create_dir_all(&sealed_dir).map_err(|e| io_error("creating", &sealed_dir, e))?;
    let sealed = seal(&ring, &store, reason, mono_ns);
    let written = write_sealed(&sealed_dir, &sealed, mono_ns)?;

    Ok(Some(Recording {
        name: written
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size_bytes: sealed.bytes.len() as u64,
        digest: format!(
            "sha256:{}",
            String::from_utf8_lossy(&skynet_sha256::hex_of(&sealed.digest))
        ),
        records: sealed.continuity.records as u64,
        damaged: sealed.continuity.damaged as u64,
    }))
}

fn write_sealed(dir: &Path, sealed: &Sealed, mono_ns: u64) -> std::io::Result<PathBuf> {
    let name = format!("{:020}-{}.mcap", mono_ns, sealed.reason.as_str());
    let path = dir.join(name);
    let mut file = File::create(&path).map_err(|e| io_error("creating", &path, e))?;
    file.write_all(&sealed.bytes)
        .map_err(|e| io_error("writing", &path, e))?;
    file.sync_all()
        .map_err(|e| io_error("flushing", &path, e))?;
    Ok(path)
}

/// The metadata a supervisor can honestly fill in today.
///
/// **The embodiment digest is real when this robot has a descriptor**, and zero when it does not.
/// Zero is readable as "no body was known" and is not a value anybody could mistake for a digest,
/// which is what lets the recorder open on a robot nobody has provisioned — §28 preallocates the
/// ring precisely so that nothing silences it.
///
/// `simulator` comes from the descriptor for the reason §56.2 gives: a benchmark result replayed
/// from a simulated recording must never be mistaken for a field one. A body that declares itself a
/// simulator says so in every recording it produces, in the header, rather than in somebody's
/// memory of which machine they ran on. An unknown body is not a simulator (§52: absent means
/// false, because the obligation is asymmetric).
///
/// **The OS image digest is still zero**, and that one is not a placeholder either: §10's
/// attestation is unwritten, so there is nothing that has established which image is running. A
/// digest of the running binary would be a different claim wearing this field's name.
pub fn supervisor_meta(layout: &Layout) -> Meta {
    let robot_id = fs::read_to_string(layout.identity().join("robot-id"))
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|_| "unprovisioned".to_string());
    let body = crate::embodiment::load(layout);
    Meta {
        robot_id,
        embodiment: body.digest_or_zero(),
        os_image: [0u8; 32],
        tier: 0,
        enforcement: 0,
        simulator: body.simulator(),
        wall_epoch_ns: 0,
        mono_ns: 0,
        channels: Vec::new(),
    }
}

// ── §38.3's other half: the audit log, mirrored onto the event channel ───────

/// How many audit entries one pass will copy into the ring.
///
/// A burst is possible — a provisioning script that trusts six keys and applies a profile writes
/// seven entries between two reconciles — and a ring is a fixed number of records. Copying an
/// unbounded number would let a configuration script *wrap the recording*, throwing away the
/// control cycles an incident review is looking for in order to record that somebody typed a
/// command. Sixteen a pass drains any realistic burst within a few seconds and cannot flood it.
const MIRROR_MAX_PER_PASS: usize = 16;

/// Read the cursor: the highest audit sequence already on the event channel.
///
/// `None` means there is no cursor file, which is not the same as zero — see [`mirror_audit`].
fn read_cursor(layout: &Layout) -> Option<u64> {
    fs::read_to_string(layout.audit_cursor())
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// What a mirroring pass did, for a caller that wants to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mirrored {
    /// Entries copied onto the event channel this pass.
    pub recorded: usize,
    /// Entries still waiting, because the per-pass cap was reached.
    pub remaining: usize,
    /// Whether this pass was the first — the one that skips history rather than replaying it.
    pub baseline: bool,
}

/// Copy audit entries the daemon has not yet recorded onto §26's event channel (§38.3).
///
/// §38.3 wants every configuration change in two places, *"a black box event and an audit-log
/// entry"*, and the two cannot be written by the same process. The ring has exactly one writer by
/// construction — two writers to a preallocated file at computed offsets is corruption rather than
/// a race that occasionally loses a record — and the changes are made by `skynetd trust add` and
/// friends, which run while the daemon is up. So the change is written to the log by whoever makes
/// it, and this copies it across on the next pass.
///
/// # The first pass skips the history, and that is the interesting decision
///
/// A robot provisioned a year ago has an audit log with hundreds of entries and a ring that holds a
/// few minutes. Replaying that log into the ring on the first pass after this code ships would
/// **wrap the recording** — the entire incident window replaced by a year of configuration history,
/// most of it about keys that have since been removed.
///
/// The two files answer different questions and the ring's answer is the perishable one. So the
/// first pass records a **baseline**: one event saying how many earlier changes exist and where to
/// read them, and the cursor jumps to the end. Nothing is lost — the audit log is the record, and
/// it is complete — and the ring keeps holding what only it can hold.
///
/// The absence of the cursor file is what distinguishes that pass, which is why [`read_cursor`]
/// returns an `Option` rather than defaulting to zero: a cursor of zero means "mirrored nothing so
/// far, start from the beginning", and a robot that legitimately reaches that state is one whose
/// log is new too.
///
/// # Failure
///
/// A write that fails stops the pass and leaves the cursor at the last entry that succeeded, so the
/// next pass retries from there. Advancing past a record that was not written would be the one
/// outcome worth avoiding: a gap in the ring that nothing marks as a gap (§47.4).
pub fn mirror_audit(layout: &Layout, recorder: &mut Recorder, mono_ns: u64) -> Mirrored {
    let audit = crate::audit::Audit::of(layout);

    let Some(cursor) = read_cursor(layout) else {
        let log = audit.read();
        let last = log.last_seq();
        if last > 0 {
            let _ = recorder.event(
                mono_ns,
                EventCode::ConfigChanged,
                "audit",
                &format!(
                    "{last} earlier configuration change(s) are in state/audit.jsonl and are not \
                     replayed here — this ring holds minutes and that file holds the life of the \
                     machine (§38.3)",
                ),
            );
        }
        write_cursor(layout, last);
        return Mirrored {
            recorded: 0,
            remaining: 0,
            baseline: true,
        };
    };

    let pending = audit.since(cursor);
    let remaining = pending.len().saturating_sub(MIRROR_MAX_PER_PASS);
    let mut recorded = 0;
    let mut reached = cursor;

    for entry in pending.into_iter().take(MIRROR_MAX_PER_PASS) {
        // The action decides the channel's vocabulary. §41.1 keeps that vocabulary closed, so an
        // unrecognised action is a configuration change rather than a new code invented on the fly.
        let code = if entry.action.starts_with("package.admit") {
            EventCode::Admission
        } else {
            EventCode::ConfigChanged
        };
        if recorder
            .event(mono_ns, code, &entry.subject, &entry.summary())
            .is_err()
        {
            break;
        }
        reached = entry.seq;
        recorded += 1;
    }

    if reached != cursor {
        write_cursor(layout, reached);
    }
    Mirrored {
        recorded,
        remaining,
        baseline: false,
    }
}

/// Advance the cursor, atomically.
///
/// A best-effort write: a cursor that could not be written means the next pass mirrors the same
/// entries again, which duplicates records rather than losing them. Duplicated is the survivable
/// direction — a reader sees the change twice and knows it happened — and the alternative is a
/// change that is in the audit log and nowhere in the recording.
fn write_cursor(layout: &Layout, seq: u64) {
    let _ = crate::observe::write_atomic(&layout.audit_cursor(), seq.to_string().as_bytes());
}
