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
