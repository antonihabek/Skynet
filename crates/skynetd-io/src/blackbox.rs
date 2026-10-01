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
//! # What this recorder writes, and what sizes the ring it writes into
//!
//! **This recorder writes events and gaps, and that has not changed.** §26's other five channels
//! carry control cycles, and cycles are `skynet-kernel-host`'s to write — this daemon has no
//! control loop in it and §33.1 keeps it off that path.
//!
//! What changed is the **shape**. This header used to say the ring "has a shape of zero joints and
//! zero channels, which is the honest description of a robot that is supervising packages and not
//! commanding a body", and that was true of a repository with no policy runtime in it. It is no
//! longer the whole answer, because the daemon starts first (§27) and therefore *creates* the
//! recording — so a ring it sized for no body is a ring the control loop that starts afterwards
//! **cannot attach to**, its cycles being wider than the records. That was §26.3's open question
//! and [`crate::shape`] is the answer to it: the caller resolves a shape from the descriptor and
//! the transport plan, and passes it here.
//!
//! So the events-only shape is now one outcome of a decision rather than a constant, and it is
//! still the right one for a robot that supervises packages and commands nothing.
//!
//! Two consequences are deliberate:
//!
//! * A sealed file from a ring nothing commanded has an empty `/sk/state`, `/sk/desired`,
//!   `/sk/verdict` and `/sk/command`, and [`Recorder::index`] says so in as many words. An incident
//!   report that looked complete because nobody had noticed the cycles were missing would be worse
//!   than no incident report.
//! * The ring is **recreated** when the shape changes, because a record's size is a function of the
//!   shape and a ring cannot hold two sizes. The recording that was there is sealed first, so
//!   installing a policy that changes the body's width costs the previous window a file rather than
//!   costing it existence.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use skynet_blackbox::record::{Body, Entry, EventCode, EventView, Gap, Shape};
use skynet_blackbox::ring::{Geometry, Meta, Ring};
use skynet_blackbox::seal::{seal, Reason, Sealed};
use skynet_json::Json;
use skynet_lock::{Joining, Role, Writership};

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
    // §26.2's writership: which of the two roles this process has, and the shared counter its
    // sequence numbers come from. Held for the recorder's life; dropping it releases the directory
    // if this process is the creator.
    writership: Writership,
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

        // §26.2, before the ring is touched. A `Ring` keeps its next sequence in memory, so two of
        // them over this file would encode two records into one slot and produce a recording that
        // is intact by every check a reader applies and wrong where it matters.
        //
        // The lock decides which process **creates** the recording rather than which one may write
        // it: a second writer attaches to the shared counter and claims sequences from it. A
        // failure to join at all is treated by the caller as "no black box" rather than as a reason
        // not to start — §33.1 means a supervisor that cannot record is still a supervisor that can
        // stop a robot.
        let joining = Writership::join(&dir, "skynetd")
            .map_err(|error| std::io::Error::other(error.to_string()))?;

        let path = dir.join("ring");

        let creating = match joining {
            Joining::Creator(creating) => creating,
            // Somebody else created this recording. An attacher may **not** create or replace the
            // ring — that would delete the window the creator is still filling — so it takes the
            // ring as it finds it.
            //
            // **And it takes any shape**, which is asymmetric with `skynet-kernel-host` and is the
            // point rather than an oversight. A record's size is a function of the joint and
            // channel counts, and this recorder writes only events and gaps: neither carries a
            // per-joint array, so both fit whatever record size the creator's body implied. A
            // control loop's cycle does not fit a ring shaped for fewer joints, which is why the
            // loop's recorder refuses a mismatch and this one does not.
            //
            // The `shape` argument is therefore the shape this process would have *created*, and
            // the ring's own is what it writes into.
            Joining::Attacher(attaching) => {
                let holder = attaching.holder().to_string();
                // Waited for, not read once — and this is the side the defect was found on. §33.1
                // starts the control loop first, so on a robot with a new `/data` this daemon
                // arrives while the loop is still writing a 64 MB ring, and a single read returned
                // `Short` and left the supervisor with no black box for the life of the process.
                // `skynet_lock::ring_file` carries the run and the reasoning.
                let store = skynet_lock::ring_file::read_when_published(
                    &path,
                    skynet_lock::ring_file::PUBLISH_WAIT,
                )
                .map_err(|e| io_error("opening", &path, e))?;
                let ring = Ring::open(&store).map_err(|error| {
                    std::io::Error::other(format!(
                        "{holder} holds this black box directory and the ring it created cannot be \
                         opened here: {error:?}"
                    ))
                })?;
                let writership = attaching
                    .attach()
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                let scratch = vec![0u8; ring.geometry().record_size];
                return Ok(Recorder {
                    ring,
                    path,
                    sealed,
                    scratch,
                    writership,
                });
            }
        };

        if let Ok(store) = fs::read(&path) {
            if let Ok(ring) = Ring::open(&store) {
                if ring.geometry().shape == shape {
                    let scratch = vec![0u8; ring.geometry().record_size];
                    let writership = creating.seed(ring.next_sequence());
                    return Ok(Recorder {
                        ring,
                        path,
                        sealed,
                        scratch,
                        writership,
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
        // Published by rename rather than written at its name: the other writer opens this path as
        // soon as its writership says somebody else created it, and a partial ring is what it used
        // to find. See `skynet_lock::ring_file`.
        skynet_lock::ring_file::publish(&path, &store)
            .map_err(|e| io_error("preallocating", &path, e))?;
        let scratch = vec![0u8; ring.geometry().record_size];
        let writership = creating.seed(ring.next_sequence());
        Ok(Recorder {
            ring,
            path,
            sealed,
            scratch,
            writership,
        })
    }

    /// Which of §26.2's two roles this recorder has.
    pub fn role(&self) -> Role {
        self.writership.role()
    }

    /// The dead holder whose writer lock this recorder reclaimed, if it reclaimed one.
    ///
    /// The ordinary case on a robot that has been restarted: `skynetd` is stopped with a signal and
    /// installs no handler, so its lock outlives it. Reported by the caller rather than swallowed,
    /// because a directory that changed hands is a fact an investigator reading the recording is
    /// entitled to (§26.2).
    pub fn reclaimed_from(&self) -> Option<&str> {
        self.writership.reclaimed_from()
    }

    /// The geometry this recording has.
    pub fn geometry(&self) -> Geometry {
        self.ring.geometry()
    }

    /// The sequence the next record written will get.
    ///
    /// Taken when §74's telemetry window opens, so the window can later be counted off the ring
    /// rather than off counters kept beside it. Counters beside the ring would be a second copy of
    /// a fact the ring already holds, and the two would disagree the first time a write failed.
    ///
    /// Off the shared counter when there is one, because with two writers the ring's own is a floor
    /// rather than the answer: it knows what *this* process has staged and nothing about what the
    /// other has claimed since.
    pub fn next_sequence(&self) -> u64 {
        self.writership
            .peek()
            .unwrap_or_else(|| self.ring.next_sequence())
    }

    /// What this daemon can see of the Safety Kernel, off §13's ring (§27, §33.1).
    ///
    /// # Why the ring, and why not systemd
    ///
    /// §27 has a `kernel` condition and `skynetd` has never supplied it, because §33.1 keeps this
    /// process away from the actuator path — CI asserts it cannot even depend on `skynet-rhal`. So
    /// the machine has derived `safe` on every robot, always, from a default rather than from an
    /// observation.
    ///
    /// The obvious source is systemd: the kernel is a unit, and this daemon already asks systemd
    /// about units. It is the wrong source, and the reason is the case that matters. **A hung
    /// kernel is an active unit.** `ActiveState=active` says a process exists, which is exactly
    /// what a robot with a wedged control loop also has, and reporting that as `live` would put
    /// §27's most consequential condition on the one signal that cannot tell the dangerous case
    /// from the healthy one.
    ///
    /// A control loop that is turning **writes a cycle every pass** (§26). That record is produced
    /// as a side effect of the kernel doing its job, it is already in a file this daemon holds, and
    /// it cannot be produced by a loop that has stopped. So liveness is *cycles recorded since the
    /// last look*, and nothing else.
    ///
    /// # Why this does not cross §33.1
    ///
    /// Nothing here asks the kernel anything, commands anything, or touches a driver: it reads a
    /// file. The inference is one-directional and fails closed — cycles seen means `Live`, and
    /// every other outcome, including an unreadable ring, means `Down`. This daemon can never
    /// *raise* a robot's operating state on a guess; the strongest thing a wrong answer here can do
    /// is hold a robot that could have been commanding, which is the direction §33.1 exists to
    /// choose.
    ///
    /// # The three answers
    ///
    /// [`KernelSighting::Unobservable`] is not a reading. A ring with no action channels cannot
    /// hold a control cycle at all (`Tally::commands`), so silence from it says nothing about a
    /// kernel — which is the state of every robot on this edition, and the reason `GET /state`
    /// reports this condition as unobserved rather than as `down`.
    /// **This recorder cannot write a cycle**, and that is worth stating where the reading is
    /// taken. §26.2 gives the supervisor the event channel and the control loop the four cycle
    /// channels, so the evidence `skynetd` reads here is evidence it is structurally incapable of
    /// manufacturing. A liveness signal a process can forge about itself is not one.
    pub fn sighting(&self, since: u64) -> KernelSighting {
        KernelSighting::from_tally(self.tally(since).as_ref())
    }

    /// What this daemon can see of the **RHAL**, off the same records (§27, §24).
    ///
    /// The companion to [`sighting`](Self::sighting), and it reads the *newest* cycle rather than
    /// a count. §27's `rhal` is a **state**, so what decides it is the last thing the loop
    /// observed, not what happened at some point in the interval: a driver that missed one read
    /// out of two hundred is not lost, and one that has answered nothing for a second is, however
    /// many good reads preceded it.
    ///
    /// The interval still matters for the other half — a robot with no cycles at all has nothing
    /// reading its body, which is `Lost` for a different reason and is reported as one.
    pub fn body(&self, since: u64) -> RhalSighting {
        match self.newest_cycle(since) {
            Err(NoCycle::Unreadable) => RhalSighting::Unobservable(
                "the ring could not be read, so nothing can be said about the driver",
            ),
            Err(NoCycle::NoChannels) => RhalSighting::Unobservable(
                "this recording has no action channels, so it holds no control cycles and cannot \
                 say whether anything is reading the body (§26, §74.3)",
            ),
            Ok(newest) => RhalSighting::of(newest.as_ref()),
        }
    }

    /// The newest control cycle at or after `since`, or why there could not be one.
    ///
    /// Three of §27's conditions are readings of this one record — `rhal`, `stop` and `guard_hold`
    /// — and each of the three had its own copy of this scan. The copies were identical apart from
    /// the sentence each returned when there was nothing to read, which is the half that has to
    /// stay per-condition: §64 asks a refusal to name what is missing, and *"nothing can be said
    /// about the driver"* and *"nothing can be said about the emergency-stop chain"* send an
    /// operator to different places. So the scan is shared and the wording is not.
    ///
    /// Each caller still scans on its own pass. They are called back to back from one reconcile
    /// pass, so they see the same newest cycle; if that ever stops being true they have to be
    /// handed one record instead, because §27's conditions are only comparable when they are
    /// readings of one instant.
    fn newest_cycle(&self, since: u64) -> Result<Option<skynet_blackbox::Cycle>, NoCycle> {
        let Ok(store) = self.read() else {
            return Err(NoCycle::Unreadable);
        };
        if self.ring.geometry().shape.channels == 0 {
            return Err(NoCycle::NoChannels);
        }
        Ok(self
            .ring
            .ordered(&store)
            .into_iter()
            .filter(|record| record.sequence >= since)
            .filter_map(|record| match record.body {
                skynet_blackbox::record::Body::Cycle(cycle) => Some(cycle),
                _ => None,
            })
            .next_back())
    }

    /// §27's **stop** condition, off the same newest cycle as [`body`](Self::body).
    ///
    /// Reads the ring once more rather than taking `body`'s record, and that is deliberate for now:
    /// the two are called back to back from one pass, so they see the same newest cycle, and
    /// threading a record between them would couple two readings whose failure modes differ. If
    /// this ever moves off the same pass, it has to take the record instead — §27's conditions are
    /// only comparable when they are readings of one instant.
    pub fn chain(&self, since: u64) -> StopSighting {
        match self.newest_cycle(since) {
            Err(NoCycle::Unreadable) => StopSighting::Unobservable(
                "the ring could not be read, so nothing can be said about the emergency-stop chain",
            ),
            Err(NoCycle::NoChannels) => StopSighting::Unobservable(
                "this recording has no action channels, so it holds no control cycles and cannot \
                 say whether the emergency-stop chain is open (§26, §74.3)",
            ),
            Ok(newest) => StopSighting::of(newest.as_ref()),
        }
    }

    /// §27's **homing** condition, off the same newest cycle as [`body`](Self::body).
    ///
    /// The sixth reading off this ring and the last of §27's eight to get one. What it is reading
    /// is new: §24 gained a two-bit homing field in the `flags` word it already had, so the record
    /// this scans is unchanged and the bits ride in on `state_flags` with no format change at all.
    pub fn homing(&self, since: u64) -> HomingSighting {
        match self.newest_cycle(since) {
            Err(NoCycle::Unreadable) => HomingSighting::Unobservable(
                "the ring could not be read, so nothing can be said about the homing state",
            ),
            Err(NoCycle::NoChannels) => HomingSighting::Unobservable(
                "this recording has no action channels, so it holds no control cycles and cannot \
                 say whether the body has been homed (§26, §74.3)",
            ),
            Ok(newest) => HomingSighting::of(newest.as_ref()),
        }
    }

    /// What the newest cycle measured of the body, for §54.2's `robot.state.*` (F147).
    pub fn body_reading(&self, since: u64) -> BodyReading {
        match self.newest_cycle(since) {
            Err(NoCycle::Unreadable) => BodyReading::Unobservable(
                "the ring could not be read, so nothing can be said about the body",
            ),
            Err(NoCycle::NoChannels) => BodyReading::Unobservable(
                "this recording has no action channels, so it holds no control cycles and no \
                 measurement of the body (§26, §74.3)",
            ),
            Ok(newest) => BodyReading::of(newest.as_ref()),
        }
    }

    /// §80's `proximity_reading`: is anything on this robot filling `cycle.nearest_person_m`?
    ///
    /// **An interval, not an instant**, and that is the one structural difference between this and
    /// every other reading in this file. `rhal`, `stop`, `guard_hold` and `homing` are states, so
    /// the newest cycle decides them. *Does a producer exist* is not a state: a detector running at
    /// 30 Hz under a control loop at 1 kHz leaves `NaN` in about thirty-two cycles out of
    /// thirty-three, and a reading taken off the newest one would report a working detector as
    /// absent on almost every pass and flicker between the two.
    ///
    /// So it counts, like [`Recorder::tally`], and reads the whole file for the same reason.
    pub fn proximity(&self, since: u64) -> ProximitySighting {
        let Ok(store) = self.read() else {
            return ProximitySighting::Unobservable(
                "the ring could not be read, so nothing can be said about the proximity reading",
            );
        };
        if self.ring.geometry().shape.channels == 0 {
            return ProximitySighting::Unobservable(
                "this recording has no action channels, so it holds no control cycles and cannot \
                 say whether anything fills the proximity reading (§26, §74.3)",
            );
        }
        ProximitySighting::of(
            self.ring
                .ordered(&store)
                .into_iter()
                .filter(|record| record.sequence >= since)
                .filter_map(|record| match record.body {
                    skynet_blackbox::record::Body::Cycle(cycle) => Some(cycle),
                    _ => None,
                }),
        )
    }

    /// §27's **guard hold** condition, off the same newest cycle as [`body`](Self::body).
    ///
    /// The fourth reading taken from this ring, and the first one that is a fact about the
    /// **kernel** rather than about the body. That difference decides the one case the other three
    /// had to rule out: a cycle whose `read_state` failed carries the previous cycle's flags and
    /// positions (§26.4), so `rhal` and `stop` refuse to read it — but the guard verdict on that
    /// cycle is not a fill. The kernel really did evaluate, really did decide, and really did
    /// withhold or forward the command. §27 asks whether a guard is holding, and that is answerable
    /// from a cycle whose body read failed, because it is a question about what the kernel did.
    pub fn guard(&self, since: u64) -> HoldSighting {
        match self.newest_cycle(since) {
            Err(NoCycle::Unreadable) => HoldSighting::Unobservable(
                "the ring could not be read, so nothing can be said about the guards",
            ),
            Err(NoCycle::NoChannels) => HoldSighting::Unobservable(
                "this recording has no action channels, so it holds no control cycles and cannot \
                 say whether a guard is holding a command (§26, §74.3)",
            ),
            Ok(newest) => HoldSighting::of(newest.as_ref()),
        }
    }

    /// Count §74's counters over the records at or after `from_sequence`.
    ///
    /// Reads the whole file, for [`Recorder::read`]'s reason, and runs once per reconcile pass
    /// rather than once per cycle.
    ///
    /// A ring that cannot be read produces **no tally rather than an empty one**: a caller handed
    /// zeroes could not tell a robot that counted nothing from a disk that would not answer, and
    /// those are the two facts §74.3 exists to keep apart.
    pub fn tally(&self, from_sequence: u64) -> Option<skynet_blackbox::tally::Tally> {
        let store = self.read().ok()?;
        Some(skynet_blackbox::tally::tally(
            &self.ring,
            &store,
            from_sequence,
        ))
    }

    /// The cycles since `from_sequence` in which a guard intervened **on a command** (§54.4, F153).
    ///
    /// `None` on the terms [`Recorder::tally`] reads as unmeasured — an unreadable ring, one that
    /// can hold no cycle, or a window with a hole in it — so an episode carries no count rather
    /// than a zero. See [`intervened_on_a_command`] for which cycles count.
    pub fn skill_interventions(&self, from_sequence: u64) -> Option<u64> {
        let store = self.read().ok()?;
        if !skynet_blackbox::tally::tally(&self.ring, &store, from_sequence).measured() {
            return None;
        }
        Some(
            skynet_blackbox::tally::ordered(&self.ring, &store)
                .iter()
                .filter(|record| record.sequence >= from_sequence)
                .filter(|record| match &record.body {
                    skynet_blackbox::record::Body::Cycle(cycle) => intervened_on_a_command(cycle),
                    _ => false,
                })
                .count() as u64,
        )
    }

    /// The ring's bytes, for a test that reads records back.
    #[cfg(test)]
    pub(crate) fn read_for_tests(&self) -> Vec<u8> {
        self.read().expect("the ring reads")
    }

    /// The ring's records, oldest first, for a test that reads records back.
    #[cfg(test)]
    pub(crate) fn ordered_for_tests(&self, store: &[u8]) -> Vec<skynet_blackbox::Record> {
        skynet_blackbox::tally::ordered(&self.ring, store)
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
        // §26.2. `next()` is `None` only for a lone creator on a platform with no shared mapping,
        // and there the ring's own counter is correct because there cannot be a second writer.
        let (sequence, offset) = match self.writership.next() {
            Some(claimed) => {
                let offset = self
                    .ring
                    .stage_claimed(&mut self.scratch, mono_ns, entry, claimed)
                    .map_err(|error| {
                        std::io::Error::other(format!("encoding a record: {error:?}"))
                    })?;
                (claimed, offset)
            }
            None => self
                .ring
                .stage(&mut self.scratch, mono_ns, entry)
                .map_err(|error| std::io::Error::other(format!("encoding a record: {error:?}")))?,
        };
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
    ///
    /// The creator's, and only the creator's (§26.2). Sealing reads the whole ring and writes it
    /// out under a digest; two processes doing that produce two windows of one recording, and the
    /// second reader is the thing `skynet-claim`'s no-seqlock argument says does not exist.
    pub fn seal(&self, reason: Reason, mono_ns: u64) -> std::io::Result<(Sealed, PathBuf)> {
        if !self.writership.may_seal() {
            return Err(std::io::Error::other(
                "this process attached to a black box another process created, and the creator is \
                 the one that seals it (§26.2). Ask it to seal, or read the window it writes.",
            ));
        }
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

    /// The most recent events in the recording, newest last (§41.3).
    ///
    /// §41.3's bundle asks for *"the last N black box events"* and nothing could produce them:
    /// [`Recorder::index`] reports the window's **shape** — how many records, over what interval,
    /// with what continuity — and a shape does not answer *what happened just before it stopped*,
    /// which is the question a support conversation opens with.
    ///
    /// # Gaps are events here, and that is the point
    ///
    /// A [`Body::Gap`] is not a [`Body::Event`] in the type system, and omitting it would be the
    /// obvious reading of "events". It would also present a **continuous list over a discontinuous
    /// recording**: two events either side of four hundred dropped records would read as
    /// consecutive, which is exactly the misreading §26 annotates gaps to prevent. So a gap appears
    /// in the sequence, in its place, saying how many records went missing and why.
    ///
    /// Cycles never appear. They are what a recording is almost entirely made of, they are the
    /// download's business (§39.1 puts the file behind a wider role than the index for that
    /// reason), and a "last 50 events" that returned fifty cycles would answer nothing.
    ///
    /// # `held` is not `returned`, and a reader needs both
    ///
    /// *"The ring holds 12,000 events and you are looking at the last 50"* and *"the ring holds
    /// 50"* are different facts about a robot, and a list that carried only its own length would
    /// let the second be read as the first — a recorder that has been running for a week looking
    /// like one that just started.
    pub fn events(&self, limit: usize) -> Json {
        let store = self.read().unwrap_or_default();
        let ordered = self.ring.ordered(&store);

        // Counted over the whole ring before anything is dropped, so `held` is about the recording
        // rather than about this request.
        let held = ordered
            .iter()
            .filter(|record| {
                matches!(record.body, Body::Event(_)) || matches!(record.body, Body::Gap(_))
            })
            .count();

        let mut rows: Vec<Json> = Vec::new();
        for record in ordered.iter().rev() {
            if rows.len() >= limit {
                break;
            }
            let mut row = Json::obj();
            row.set("sequence", Json::Num(record.sequence as f64));
            row.set("mono_ns", Json::Num(record.mono_ns as f64));
            match &record.body {
                Body::Event(event) => {
                    row.set("kind", Json::from("event"));
                    row.set("code", Json::from(event.code.as_str()));
                    row.set("subject", Json::Str(event.subject.clone()));
                    row.set("detail", Json::Str(event.detail.clone()));
                }
                Body::Gap(gap) => {
                    row.set("kind", Json::from("gap"));
                    row.set("code", Json::from(gap.reason.as_str()));
                    row.set("dropped", Json::Num(gap.dropped as f64));
                    row.set("first_tick", Json::Num(gap.first_tick as f64));
                }
                Body::Cycle(_) | Body::Frame(_) => continue,
            }
            rows.push(row);
        }
        // Newest last, because a list somebody reads top to bottom should end where the robot is
        // now. Collected newest-first above so the `limit` takes the *recent* end rather than the
        // oldest records still in the ring.
        rows.reverse();

        let mut document = Json::obj();
        document.set("events", Json::Arr(rows.clone()));
        document.set("returned", Json::Num(rows.len() as f64));
        document.set("held", Json::Num(held as f64));
        document.set("limit", Json::Num(limit as f64));
        document
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

        // §90.2: which cycles ran on a remote policy's proposals, from which session and model.
        // Present only when the recording names a remote session, so a robot that has never run
        // one is not shown an empty section that reads like a measurement.
        let held = skynet_blackbox::tally::ordered(&self.ring, &store);
        let (sessions, unreadable) = skynet_blackbox::remote::sessions(&held);
        if !sessions.is_empty() || unreadable > 0 {
            let (counts, local) = skynet_blackbox::remote::cycles_by_session(&held, &sessions);
            let mut remote = Json::obj();
            let mut listed = Json::arr();
            for (entry, cycles) in sessions.iter().zip(counts.iter()) {
                let mut item = Json::obj();
                item.set("session", Json::Str(entry.session.clone()));
                item.set(
                    "digest",
                    entry
                        .digest
                        .as_ref()
                        .map_or(Json::Null, |digest| Json::Str(digest.clone())),
                );
                // Strings, because a stamp past a hundred days of uptime is not exact as a double.
                let mark = |mark: Option<skynet_blackbox::remote::CommandMark>| {
                    mark.map_or(Json::Null, |mark| {
                        let mut at = Json::obj();
                        at.set("seq", Json::Str(mark.seq.to_string()));
                        at.set("stamp_ns", Json::Str(mark.stamp_ns.to_string()));
                        at
                    })
                };
                item.set("first", mark(entry.first));
                item.set("last", mark(entry.last));
                item.set("cycles", Json::Num(*cycles as f64));
                listed.push(item);
            }
            remote.set("sessions", listed);
            remote.set("other_cycles", Json::Num(local as f64));
            remote.set("unreadable", Json::Num(unreadable as f64));
            document.set("remote", remote);
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

/// What a daemon could see of the RHAL in one interval (§27's `rhal` condition).
///
/// Derived from the **newest** control cycle in the interval, because §27's `rhal` is a state and
/// a state is what the last observation says. §24 gives three facts to read it from, and each maps
/// onto exactly one of `RhalState`'s three values — which is why this is a mapping rather than a
/// judgement:
///
/// | What the record says | State | Why |
/// |---|---|---|
/// | no cycle at all | `Lost` | nothing is reading the body |
/// | `read_result` negative | `Lost` | the driver did not answer this cycle |
/// | `read_result` unrecorded (schema 1) | `Lost` | an older recording cannot say, and this fails closed |
/// | `RHAL_DEGRADED` in the flags | `Degraded` | §24: *"running, but something is wrong"* |
/// | otherwise | `Present` | the driver answered and reported nothing wrong |
///
/// **`RHAL_FAULT` is deliberately not `Lost`.** §24 defines it as *"the drive has faulted; commands
/// are being ignored"* — which is a fact about the **body**, not about the RHAL. A faulted drive
/// still answers reads, so the kernel has not lost it, and §27's `rhal_lost` is the row that says
/// *"nothing else can be known"* — no joint state, no estop reading, no guard that can evaluate.
/// Mapping a faulted drive onto it would tell an operator the robot cannot be observed at the exact
/// moment it can be observed and is telling them something. What a fault does belongs to §7's
/// guards and to the fault code the record carries verbatim, not to this condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RhalSighting {
    /// Nothing could be read, with the reason. Not a reading.
    Unobservable(&'static str),
    /// A cycle was read, and this is what it said.
    Seen {
        /// §27's condition.
        state: skynet_contracts::opstate::RhalState,
        /// What `read_state` returned on that cycle: `None` when it succeeded.
        errno: Option<i32>,
        /// The driver's own fault code, verbatim (§24). Zero when it reported none.
        fault_code: u32,
    },
    /// The recording can hold cycles and none was written in this interval.
    Silent,
}

impl RhalSighting {
    /// Map the newest cycle onto §27's condition. See the type's own table.
    pub fn of(newest: Option<&skynet_blackbox::Cycle>) -> RhalSighting {
        use skynet_blackbox::record::ReadOutcome;
        use skynet_contracts::opstate::RhalState;

        let Some(cycle) = newest else {
            return RhalSighting::Silent;
        };
        let (state, errno) = match cycle.read {
            ReadOutcome::Failed(code) => (RhalState::Lost, Some(code)),
            // A recording from before §26.4. It cannot say, and a reader that took silence for
            // success would be the failure that field was added to prevent — so `Lost`, which
            // holds the robot rather than releasing it.
            ReadOutcome::Unrecorded => (RhalState::Lost, None),
            ReadOutcome::Ok if cycle.state_flags & RHAL_DEGRADED != 0 => {
                (RhalState::Degraded, None)
            }
            ReadOutcome::Ok => (RhalState::Present, None),
        };
        RhalSighting::Seen {
            state,
            errno,
            fault_code: cycle.fault_code,
        }
    }

    /// §27's condition. Anything that is not a good read is `Lost`.
    pub fn state(&self) -> skynet_contracts::opstate::RhalState {
        match self {
            RhalSighting::Seen { state, .. } => *state,
            _ => skynet_contracts::opstate::RhalState::Lost,
        }
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, RhalSighting::Unobservable(_))
    }

    /// What a console shows beside the condition.
    pub fn detail(&self) -> String {
        match self {
            RhalSighting::Unobservable(why) => (*why).to_string(),
            RhalSighting::Silent => {
                "no control cycle was recorded in the last interval, so nothing has read the body"
                    .to_string()
            }
            RhalSighting::Seen {
                state,
                errno: Some(code),
                ..
            } => format!(
                "the newest cycle's `read_state` returned {code} (§24), so the driver is {}",
                state.as_str()
            ),
            RhalSighting::Seen {
                state, fault_code, ..
            } if *fault_code != 0 => format!(
                "the driver answered and reported fault code {fault_code}, verbatim (§24). The \
                 RHAL is {}: a faulted drive still answers reads, which is why a fault is not a \
                 loss",
                state.as_str()
            ),
            RhalSighting::Seen { state, .. } => {
                format!(
                    "the newest cycle read the body and reported it {}",
                    state.as_str()
                )
            }
        }
    }
}

/// §24's `RHAL_DEGRADED`: running, but something is wrong — thermal, a dead encoder.
const RHAL_DEGRADED: u16 = 1 << 2;

/// §24's `SKR_FLAG_ESTOP`: an emergency stop is asserted.
///
/// Restated here rather than imported, for [`RHAL_DEGRADED`]'s reason and CI's: §33.1 forbids this
/// crate an edge to `skynet-rhal`, and a transitive dependency reaches an actuator exactly as well
/// as a direct one. What crosses the line is the **number**, out of a record this process already
/// holds — the same arrangement §26 makes for every other field in a cycle.
/// Why [`Recorder::newest_cycle`] had no record to hand back.
///
/// Two reasons and not one, because they are opposite facts about a robot and §74.3 spends a whole
/// rule on keeping that pair apart: a ring nobody can read is a fault, and a ring with no action
/// channels is every robot on this edition working exactly as designed (§26.2). Collapsing them
/// would make a disk failure indistinguishable from a shipping configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoCycle {
    /// The recording could not be opened or decoded.
    Unreadable,
    /// The recording is shaped for events only, so a control cycle cannot be in it.
    NoChannels,
}

/// What a daemon could see of §27's **guard hold** condition (`Conditions::guard_hold`).
///
/// The fourth reading off §13's ring, and a *state* like [`RhalSighting`] and [`StopSighting`]:
/// "is a guard holding" is a question about now, so the newest cycle decides it.
///
/// # Why a failed read does not stop this one
///
/// [`StopSighting`] refuses a cycle whose `read_state` failed, because §24's flags in such a record
/// are the previous cycle's (§26.4) and reading them would be reporting a fill as an observation.
/// The guard verdict in the same record is **not** a fill. `decide` runs on every cycle, failed
/// read or not, and what it produces — `Hold`, `Clamp`, a stop, or nothing — is what the kernel
/// did with the command that cycle. So this reads a record the other two refuse, and the split is
/// the point: `rhal` and `stop` are facts about the **body**, and this is a fact about the
/// **kernel**.
///
/// What the verdict was *computed from* on such a cycle is a fair question and a different one. The
/// guards saw the driver's last buffers and a zeroed flag word, so a workspace guard may have held
/// on a stale position. It still held: the command was withheld, the robot did not move, and an
/// operator asking why their robot is not moving needs that answer rather than a philosophical one
/// about the quality of its inputs.
///
/// # `Advisory` is a hold that did not happen
///
/// §7.4 lets a tier that cannot promise a clamp run **advisory**: the guards evaluate, the verdict
/// is recorded, and the command is forwarded anyway. Every robot on this edition is one
/// (`limits::ROBOT_TIER` is guest), so `Some(FaultResponse::Hold)` in a record here almost always
/// describes a hold that was demanded and not applied.
///
/// Mapping that onto §27's `guard_hold` would tell an operator their robot is held at the moment
/// it is moving — which is the failure §7.4 and §42 exist to prevent, and which this repository
/// already caught once inside `decide` itself, where the `Hold` arm zeroed the mask on every
/// verdict including advisory ones. So it is a variant of its own: not observed-as-clear, because
/// a guard did fire and somebody should see that; not held, because nothing was withheld.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldSighting {
    /// Nothing could be read, with the reason. Not a reading.
    Unobservable(&'static str),
    /// The newest cycle's verdict was a hold, and it was enforced.
    Held,
    /// The newest cycle's verdict was a hold and the verdict was advisory, so nothing was withheld.
    Advisory,
    /// The newest cycle demanded no hold.
    Clear,
}

impl HoldSighting {
    /// Map the newest cycle onto §27's condition.
    pub fn of(newest: Option<&skynet_blackbox::Cycle>) -> HoldSighting {
        match newest {
            None => HoldSighting::Unobservable(
                "no control cycle was recorded in the last interval, so no guard has run",
            ),
            // `skynet_blackbox`'s re-export rather than `skynet_kernel` directly. This crate's
            // dependency list is a safety property — it deliberately does not carry `skynet-rhal`,
            // and CI asserts the edge is absent — so a new edge into the kernel to name one enum
            // variant would be paying in the wrong currency for a convenience.
            Some(cycle) => match cycle.response {
                Some(skynet_blackbox::FaultResponse::Hold) if cycle.advisory => {
                    HoldSighting::Advisory
                }
                Some(skynet_blackbox::FaultResponse::Hold) => HoldSighting::Held,
                // Everything else is not a hold, and the list is deliberately not enumerated: a
                // clamp forwards a reduced command, a stop is §27's own row above this one, and
                // `None` is a cycle where the guards passed or nothing was posted. None of them
                // withholds on the strength of a guard hold, which is the only thing this reports.
                _ => HoldSighting::Clear,
            },
        }
    }

    /// §27's condition. True only for a hold that was **enforced**.
    pub fn held(&self) -> bool {
        matches!(self, HoldSighting::Held)
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, HoldSighting::Unobservable(_))
    }

    /// What was seen, in a sentence.
    pub fn detail(&self) -> String {
        match self {
            HoldSighting::Unobservable(why) => (*why).to_string(),
            HoldSighting::Held => "a guard held the command on the newest cycle: the kernel \
                                   withheld it on every channel and the last safe command stands"
                .to_string(),
            HoldSighting::Advisory => {
                "a guard demanded a hold on the newest cycle and the verdict \
                                       was advisory, so the command was forwarded unchanged. This \
                                       tier observes and reports; it does not enforce (§7.4)"
                    .to_string()
            }
            HoldSighting::Clear => {
                "the newest cycle ran the guards and none of them held the command".to_string()
            }
        }
    }
}

const RHAL_ESTOP: u16 = 1 << 0;

/// §24's `SKR_HOMING_MASK`: bits 4-5 of the flags word, restated for [`RHAL_ESTOP`]'s reason.
///
/// The value `0` in this field is `SKR_HOMING_UNREPORTED` and is not one of §27's homing states,
/// which is why [`HomingSighting`] has a variant the contract enum does not.
const RHAL_HOMING_MASK: u16 = 0b11 << RHAL_HOMING_SHIFT;

/// Where §24's homing field starts in the flags word.
const RHAL_HOMING_SHIFT: u16 = 4;

/// `SKR_HOMING_NOT_REQUIRED`, already shifted.
const RHAL_HOMING_NOT_REQUIRED: u16 = 1 << RHAL_HOMING_SHIFT;

/// `SKR_HOMING_PENDING`, already shifted.
const RHAL_HOMING_PENDING: u16 = 2 << RHAL_HOMING_SHIFT;

/// `SKR_HOMING_COMPLETE`, already shifted.
const RHAL_HOMING_COMPLETE: u16 = 3 << RHAL_HOMING_SHIFT;

/// Whether anything on this robot is producing §7.2's proximity reading, over one interval.
///
/// # The gap this closes, and the one it does not
///
/// §80 raised `inert` for a guard *"whose every declared input resolves and whose per-cycle reading
/// never arrives"*, and built it against a **constant**: `available.proximity_reading` was `false`
/// in both the daemon and the kernel host, each with a comment saying *"the day a producer exists,
/// this is the line that changes"*. That is a correct answer on this edition and it is not a
/// reading, and the difference shows up the moment a producer does exist — a robot would go on
/// drawing the guard as inert with a detector filling the field every cycle.
///
/// It also closes half of the gap [`crate::limits::GuardsSighting`] names in its own header: *"a
/// declared sensor that fails at runtime is a per-cycle fact and belongs with the ring readings"*.
/// A detector that was filling the field and stops now moves the guard from active to **inert**
/// within an interval, which is the direction that matters: a console stops drawing protection the
/// robot no longer has. What it still cannot do is say *why* it stopped — that is the vendor's
/// fault code, and §24 does not carry one per sensor.
///
/// # Why `filled == 0` is not "the sensor is broken"
///
/// It is *nothing produced a reading in this interval*, which on a robot with no vision path is
/// every interval and entirely correct. The guard is inert either way and the remedy differs, so
/// the count is carried rather than a boolean: an operator seeing 0 of 4,000 cycles is looking at
/// a different problem from one seeing 0 of 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProximitySighting {
    /// Nothing could be counted, with the reason. Not a reading.
    Unobservable(&'static str),
    /// Cycles were counted, and this many of them carried a reading.
    Counted {
        /// Control cycles in the interval whose body read succeeded.
        cycles: u64,
        /// How many of those carried a number rather than `NaN`.
        filled: u64,
    },
}

impl ProximitySighting {
    /// Count the interval's cycles, and how many of them carried a reading.
    ///
    /// Pure, for [`KernelSighting::from_tally`]'s reason: only a control loop can put a cycle in a
    /// ring (§26.2), so a rule that could be exercised only through a disk would be one this
    /// crate's own suite could never reach.
    pub fn of(cycles: impl Iterator<Item = skynet_blackbox::Cycle>) -> ProximitySighting {
        let mut counted = 0u64;
        let mut filled = 0u64;
        for cycle in cycles {
            // A failed read is counted in neither, for §26.4's reason one field along: the fill
            // carries the previous cycle's value, so a `NaN` there is not evidence that nothing
            // produced a reading and a number there is not evidence that something did.
            if !cycle.read.measured() {
                continue;
            }
            counted += 1;
            if !cycle.nearest_person_m.is_nan() {
                filled += 1;
            }
        }
        if counted == 0 {
            return ProximitySighting::Unobservable(
                "no control cycle was recorded in the last interval, so nothing has read the \
                 proximity sensor",
            );
        }
        ProximitySighting::Counted {
            cycles: counted,
            filled,
        }
    }

    /// Whether a producer exists. Everything that is not a positive count is `false`.
    ///
    /// Fail-closed, and here that means **the guard is drawn as inert rather than as protection**,
    /// which is the safe direction: §7.2's partition is what a console renders, and an active
    /// proximity guard is a claim that the robot is watching for people.
    pub fn arriving(&self) -> bool {
        matches!(self, ProximitySighting::Counted { filled, .. } if *filled > 0)
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, ProximitySighting::Unobservable(_))
    }

    /// What was seen, in a sentence.
    pub fn detail(&self) -> String {
        match self {
            ProximitySighting::Unobservable(why) => (*why).to_string(),
            ProximitySighting::Counted { cycles, filled } if *filled == 0 => format!(
                "{cycles} control cycle(s) were recorded in this interval and none carried a \
                 proximity reading, so nothing on this robot fills `cycle.nearest_person_m`"
            ),
            ProximitySighting::Counted { cycles, filled } => format!(
                "{filled} of {cycles} control cycle(s) carried a proximity reading, so a producer \
                 exists. A detector slower than the control loop fills a minority of cycles and \
                 that is the expected shape, not a fault"
            ),
        }
    }
}

/// What a daemon could see of §27's **homing** condition in one interval.
///
/// # The last of the eight, and the one whose reason was wrong rather than stale
///
/// The other seven closed by finding that the evidence was already written down somewhere this
/// process could read. This one closed by finding that the *reason* it could not be read was a
/// conclusion nobody had rechecked: §73 said §24 gave the ABI no flag for homing, which was true,
/// and inferred that closing it needed a vendor to recompile, which did not follow. §24's `flags`
/// is a `u16` carrying four flags and twelve spare bits, and a field added to a bitwise-or word
/// moves no offset, breaks no driver and does not bump [`SKYNET_RHAL_ABI_VERSION`.
/// The OS could define the bits and read them at any point in the last several months.
///
/// # Four answers, and the fourth is why this is not a boolean
///
/// §27's [`HomingState`](skynet_contracts::opstate::HomingState) has three values and the field's
/// zero is a fourth thing: *this driver does not answer*. Folding that onto `Pending` was the
/// tempting simplification and it is the one §7.2 forbids — the machine holds the robot either
/// way, so the only thing the distinction costs is a sentence, and the only thing it buys is that
/// an operator with an absolute-encoder robot is not sent to run a homing routine that does not
/// exist.
///
/// The variant therefore carries §27's own enum rather than a second spelling of it. Only the wire
/// **numbers** are restated in this crate, for [`RHAL_ESTOP`]'s reason: §33.1 forbids an edge to
/// `skynet-rhal` and a number out of a record this process already holds is what crosses instead.
///
/// # Why a failed read is refused
///
/// The same rule as [`StopSighting`], for the same reason and off the same bytes: §26.4 fills a
/// failed cycle's flags with the previous cycle's, so the homing field in such a record is a copy
/// rather than a reading. It parts company with [`HoldSighting`] here, which reads exactly those
/// records — the guard verdict on a failed cycle is what the kernel *did*, and this is a claim
/// about what the driver *saw*.
///
/// # It never reports homing as it happens
///
/// §27 has three homing states and no fourth for *homing right now*. A driver mid-routine reports
/// `Pending` — positions are not yet trustworthy, which is exactly true while an axis is seeking —
/// and the transition to `Complete` is the only edge there is. That is a limit of §27's vocabulary
/// rather than of this reading, and it is the fail-closed direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomingSighting {
    /// Nothing could be read, with the reason. Not a reading.
    Unobservable(&'static str),
    /// A cycle was read and the driver declined to answer — `SKR_HOMING_UNREPORTED`.
    ///
    /// Distinct from [`HomingSighting::Unobservable`] on purpose: this robot's ring is readable,
    /// its loop is turning and its driver reported a state. The one thing it did not report is
    /// this, and that is a fact about the driver rather than about the evidence.
    Unreported,
    /// A cycle was read, and the driver answered with one of §27's three states.
    Reported(skynet_contracts::opstate::HomingState),
}

impl HomingSighting {
    /// Map the newest cycle onto §27's condition.
    pub fn of(newest: Option<&skynet_blackbox::Cycle>) -> HomingSighting {
        match newest {
            None => HomingSighting::Unobservable(
                "no control cycle was recorded in the last interval, so nothing has read the \
                 homing state",
            ),
            Some(cycle) if !cycle.read.measured() => HomingSighting::Unobservable(
                "the newest cycle's read of the body failed, so its flags are a fill value and \
                 not a reading of the homing state (§26.4)",
            ),
            Some(cycle) => {
                use skynet_contracts::opstate::HomingState;
                match cycle.state_flags & RHAL_HOMING_MASK {
                    RHAL_HOMING_NOT_REQUIRED => HomingSighting::Reported(HomingState::NotRequired),
                    RHAL_HOMING_PENDING => HomingSighting::Reported(HomingState::Pending),
                    RHAL_HOMING_COMPLETE => HomingSighting::Reported(HomingState::Complete),
                    // `SKR_HOMING_UNREPORTED`, and the only remaining value the mask can yield.
                    _ => HomingSighting::Unreported,
                }
            }
        }
    }

    /// §27's condition.
    ///
    /// Everything that is not a reported state collapses onto
    /// [`HomingState::Pending`](skynet_contracts::opstate::HomingState::Pending), which is the
    /// value that holds the robot and is already `HomingState`'s own default. That is the
    /// fail-closed direction and the only one available: a machine that cannot establish whether
    /// joint positions mean anything must not be commanded off them.
    pub fn state(&self) -> skynet_contracts::opstate::HomingState {
        use skynet_contracts::opstate::HomingState;
        match self {
            HomingSighting::Reported(state) => *state,
            HomingSighting::Unreported | HomingSighting::Unobservable(_) => HomingState::Pending,
        }
    }

    /// Whether this was an observation at all.
    ///
    /// [`HomingSighting::Unreported`] counts as **observed**, and that is the one judgement in this
    /// type worth arguing. This daemon looked, the ring answered, and what came back was a driver
    /// declining to say — which is a measurement of the driver. Counting it unobserved would put
    /// every robot on a v1 driver back where the condition started and lose the distinction the
    /// two-bit field was added to make.
    pub fn observed(&self) -> bool {
        !matches!(self, HomingSighting::Unobservable(_))
    }

    /// What was seen, in a sentence.
    pub fn detail(&self) -> String {
        match self {
            HomingSighting::Unobservable(why) => (*why).to_string(),
            HomingSighting::Unreported => {
                "the newest cycle read the body and the driver reported no homing state. §24's \
                 field is two bits in the flags word and a driver built before it existed leaves \
                 them zero, so this is a driver that has not been updated rather than a robot that \
                 needs homing. The robot is held either way"
                    .to_string()
            }
            HomingSighting::Reported(skynet_contracts::opstate::HomingState::NotRequired) => {
                "the driver reports that this body needs no homing: its joints carry absolute \
                 encoders, so the positions on every cycle mean what they say"
                    .to_string()
            }
            HomingSighting::Reported(skynet_contracts::opstate::HomingState::Pending) => {
                "the driver reports the body as not yet homed, so joint positions are not \
                 trustworthy and nothing may be commanded from them"
                    .to_string()
            }
            HomingSighting::Reported(skynet_contracts::opstate::HomingState::Complete) => {
                "the driver reports the body as homed on the newest cycle".to_string()
            }
        }
    }
}

/// Whether a guard intervened in this cycle on a command somebody sent (§54.4, F153).
///
/// Every tripped cycle but one kind: the **watchdog alone**. §7.2's watchdog trips when no fresh
/// command has arrived, and it short-circuits — nothing else is evaluated on such a cycle — so a
/// cycle whose only trip is the watchdog is the kernel holding the arm because nothing is
/// commanding it, not a guard refusing something a skill asked for.
///
/// The distinction is the whole of F153. A loop keeps turning after a policy stops, and from then
/// on every cycle holds the last command stale and trips the watchdog. §54.4's runtime counted
/// every trip in the interval as the running skill's, so a skill started after any other policy
/// had run was aborted `guard_tripped` on its first pass — for cycles recorded before its process
/// had posted anything — and a skill that returned cleanly was aborted in `verifying` for the hold
/// that followed it. A skill that stalls mid-motion is not lost by this: it is §54.4's timeout
/// row, which is the row that describes it.
pub fn intervened_on_a_command(cycle: &skynet_blackbox::Cycle) -> bool {
    cycle.intervened()
        && !cycle
            .tripped
            .iter()
            .all(|guard| *guard == skynet_blackbox::Guard::Watchdog)
}

/// What the newest cycle measured of the body: §54.2's `robot.state.*` branches (F147).
///
/// The seventh reading off the same newest cycle, and the first that is not one of §27's
/// conditions. It is the joint positions, velocities and efforts the driver returned and the flags
/// word beside them — the facts a skill's preconditions name and its success criteria are judged
/// over (§54.3). They were in the record the whole time: `live_inputs` left them absent with a
/// comment waiting for *"§26.2's cycle channels to reach this side"*, and they had, for four other
/// readings. Nothing here crosses §33.1 — it is a file the kernel wrote, read by a process that
/// holds it.
///
/// # A failed read is not a reading
///
/// §26.4 carries the previous cycle's positions into a cycle whose read failed, so the numbers in
/// such a record are a copy. A precondition that read them would be answered by a driver that did
/// not answer, which is [`StopSighting`]'s rule one field along — so the whole reading is refused
/// rather than filled, and every `robot.state.*` path is absent for that pass.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyReading {
    /// Nothing to read, with the reason.
    Unobservable(&'static str),
    /// What the driver returned on the newest cycle, in the driver's joint order.
    Measured {
        /// Measured positions.
        position: Vec<f32>,
        /// Measured velocities; `NaN` where the body does not measure them.
        velocity: Vec<f32>,
        /// Measured efforts; `NaN` where the body does not measure them.
        effort: Vec<f32>,
        /// §24's flags word.
        flags: u16,
        /// §24's vendor fault code, verbatim.
        fault_code: u32,
        /// `x y z qx qy qz qw` as the driver reported it, all `NaN` when the base is not
        /// localised (§24). Read by [`BodyReading::base_pose`], which says when it is a pose.
        base_pose: [f32; 7],
    },
}

impl BodyReading {
    /// Read the newest cycle.
    pub fn of(newest: Option<&skynet_blackbox::Cycle>) -> BodyReading {
        match newest {
            None => BodyReading::Unobservable(
                "no control cycle was recorded in the last interval, so nothing has read the body",
            ),
            Some(cycle) if !cycle.read.measured() => BodyReading::Unobservable(
                "the newest cycle's read of the body failed, so its positions are the previous \
                 cycle's carried forward and not a measurement (§26.4)",
            ),
            Some(cycle) => BodyReading::Measured {
                position: cycle.position.clone(),
                velocity: cycle.velocity.clone(),
                effort: cycle.effort.clone(),
                flags: cycle.state_flags,
                fault_code: cycle.fault_code,
                base_pose: cycle.base_pose,
            },
        }
    }

    /// Whether the newest cycle was a measurement.
    pub fn observed(&self) -> bool {
        matches!(self, BodyReading::Measured { .. })
    }

    /// How many joints the measurement carries, when there is one.
    pub fn measured_joints(&self) -> Option<usize> {
        match self {
            BodyReading::Measured { position, .. } => Some(position.len()),
            BodyReading::Unobservable(_) => None,
        }
    }

    /// What a console shows beside the joints.
    pub fn detail(&self) -> String {
        match self {
            BodyReading::Unobservable(why) => (*why).to_string(),
            BodyReading::Measured { .. } => {
                "the newest control cycle's read of the body: what the driver returned, as the \
                 loop recorded it"
                    .to_string()
            }
        }
    }

    /// §54.2's `robot.state.base_pose`, off the newest cycle. `None` unless the newest cycle was a
    /// measurement **and** the driver reported a pose that is one.
    ///
    /// §24 says a base that is not localised reports all `NaN`, and a pose is refused when any of
    /// its seven numbers is not finite: a position with no orientation is the half-fact
    /// [`skynet_contracts::live::BasePose`] exists to rule out. A quaternion that is not a rotation
    /// is refused too — a driver that wrote zeros where §24 asks for `NaN` would otherwise have the
    /// robot reported at the origin facing nowhere, and a precondition about being docked would
    /// read it as a place. The tolerance is `f32`'s, not a judgement about the driver.
    pub fn base_pose(&self) -> Option<skynet_contracts::live::BasePose> {
        let BodyReading::Measured { base_pose, .. } = self else {
            return None;
        };
        if base_pose.iter().any(|number| !number.is_finite()) {
            return None;
        }
        let norm_squared: f64 = base_pose[3..7]
            .iter()
            .map(|component| f64::from(*component) * f64::from(*component))
            .sum();
        if (norm_squared - 1.0).abs() > 1e-3 {
            return None;
        }
        Some(skynet_contracts::live::BasePose {
            xyz: [
                f64::from(base_pose[0]),
                f64::from(base_pose[1]),
                f64::from(base_pose[2]),
            ],
            quaternion: [
                f64::from(base_pose[3]),
                f64::from(base_pose[4]),
                f64::from(base_pose[5]),
                f64::from(base_pose[6]),
            ],
        })
    }

    /// §54.2's `robot.state.drive`, off the flags word. `None` where nothing was measured.
    pub fn drive(&self) -> Option<skynet_contracts::live::DriveState> {
        let BodyReading::Measured {
            flags, fault_code, ..
        } = self
        else {
            return None;
        };
        Some(skynet_contracts::live::DriveState {
            estop: flags & RHAL_ESTOP != 0,
            degraded: flags & RHAL_DEGRADED != 0,
            braked: flags & RHAL_BRAKED != 0,
            homing: match flags & RHAL_HOMING_MASK {
                RHAL_HOMING_NOT_REQUIRED => "not_required",
                RHAL_HOMING_PENDING => "pending",
                RHAL_HOMING_COMPLETE => "complete",
                _ => "unreported",
            }
            .to_string(),
            // A zero is a healthy drive's answer and is published as one (`DriveState`'s own rule).
            fault_code: Some(f64::from(*fault_code)),
        })
    }

    /// §54.2's `robot.state.joints`, `velocity` and `effort`, named by the descriptor's joints.
    ///
    /// Empty when nothing was measured, and empty when the record and the descriptor disagree about
    /// how many joints the body has. The names are the descriptor's and the numbers are the
    /// driver's, and a count that differs means one of them is describing another body — pairing
    /// them by position would put the wrist's angle under the elbow's name, which is worse than a
    /// precondition refusing because the path is absent.
    ///
    /// A non-finite position is passed through as it is, for §54.2's own context builder to report
    /// as unreadable by name. A non-finite velocity or effort is the body not measuring it (§24),
    /// so it is absent rather than zero.
    pub fn joints(&self, names: &[&str]) -> Vec<skynet_contracts::live::JointReading> {
        let BodyReading::Measured {
            position,
            velocity,
            effort,
            ..
        } = self
        else {
            return Vec::new();
        };
        if position.len() != names.len() {
            return Vec::new();
        }
        let finite = |values: &[f32], index: usize| {
            values
                .get(index)
                .map(|value| f64::from(*value))
                .filter(|value| value.is_finite())
        };
        names
            .iter()
            .enumerate()
            .map(|(index, name)| skynet_contracts::live::JointReading {
                name: (*name).to_string(),
                position: f64::from(position[index]),
                velocity: finite(velocity, index),
                effort: finite(effort, index),
            })
            .collect()
    }
}

/// §24's `SKR_FLAG_BRAKED`: holding brakes are engaged. Restated for [`RHAL_ESTOP`]'s reason.
const RHAL_BRAKED: u16 = 1 << 3;

/// What a daemon could see of §27's **stop** condition in one interval.
///
/// The third reading off the same records and the same interval as [`KernelSighting`] and
/// [`RhalSighting`], and it is a *state* like the RHAL rather than a count like the kernel: what
/// decides it is the newest cycle, because "is the chain open" is a question about now.
///
/// # Why this is a cause of its own and not `operator`
///
/// §24 gives the hardware chain **one bit**. It says the chain is open; it does not say what opened
/// it, and a light curtain, a door interlock and a safety relay noticing itself fail all open the
/// same chain as the button on the chassis. [`StopCause::Operator`] means *a person asked*, so
/// mapping this onto it would put a name on an event that has none — and §64 makes a message that
/// misdescribes what happened a defect rather than a wording choice.
///
/// # Why it never reports the stop as finished
///
/// §27 pairs a stop with `stop_settled`, and for a hardware stop **no edition can observe it**.
/// The bit says the chain is open and says nothing about whether the body has come to rest, so a
/// daemon setting `stop_settled` here would be claiming to have watched a robot stop moving. It
/// stays false, the machine derives `stopping`, and the consequence is stated rather than hidden:
/// a robot whose chain has been open for an hour goes on reporting `stopping`, because that is
/// the strongest thing this evidence supports. Fail-closed in the only direction that is safe —
/// it can hold a robot and can never release one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopSighting {
    /// Nothing could be read, with the reason. Not a reading.
    Unobservable(&'static str),
    /// A cycle was read, and the chain was open on it.
    Asserted,
    /// A cycle was read, and it was not.
    Clear,
}

impl StopSighting {
    /// Map the newest cycle onto §27's condition.
    pub fn of(newest: Option<&skynet_blackbox::Cycle>) -> StopSighting {
        match newest {
            None => StopSighting::Unobservable(
                "no control cycle was recorded in the last interval, so nothing has read the \
                 emergency-stop chain",
            ),
            // A cycle whose read failed carries the *previous* cycle's flags (§26.4), so the bit in
            // it is not a reading of the chain. Reporting `Clear` off a fill would be the exact
            // failure §26.4 was written to end, one field further along — and reporting `Asserted`
            // off one would raise a stop nobody observed.
            Some(cycle) if !cycle.read.measured() => StopSighting::Unobservable(
                "the newest cycle's read of the body failed, so its flags are a fill value and \
                 not a reading of the emergency-stop chain (§26.4)",
            ),
            Some(cycle) if cycle.state_flags & RHAL_ESTOP != 0 => StopSighting::Asserted,
            Some(_) => StopSighting::Clear,
        }
    }

    /// §27's condition. `None` is *no stop is commanded*, never *no reading*.
    pub fn cause(&self) -> Option<skynet_contracts::opstate::StopCause> {
        match self {
            StopSighting::Asserted => Some(skynet_contracts::opstate::StopCause::Estop),
            // Unobservable collapses onto `Clear` **for the machine**, and that is not a silent
            // default: §27 is already holding such a robot through `rhal`, which reads the same
            // record and answers `Lost` wherever this answers `Unobservable`. The distinction is
            // kept for the *reader* — `observed()` below — where it is the difference between a
            // chain somebody looked at and one nobody did.
            _ => None,
        }
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, StopSighting::Unobservable(_))
    }

    /// What was seen, in a sentence.
    pub fn detail(&self) -> String {
        match self {
            StopSighting::Unobservable(why) => (*why).to_string(),
            StopSighting::Asserted => "the emergency-stop chain is open on the newest cycle. §24 \
                                       gives it one bit, so what opened it is not knowable from \
                                       here — a light curtain and a door interlock open the same \
                                       chain as the button"
                .to_string(),
            StopSighting::Clear => {
                "the newest cycle read the body and the emergency-stop chain was closed".to_string()
            }
        }
    }
}

/// What a daemon could see of the Safety Kernel in one interval (§27's `kernel` condition).
///
/// Three answers rather than a boolean, because *"the ring said nothing"* and *"there is no ring
/// that could have said anything"* are different facts and only the first is an observation. §27
/// treats both as `down`, correctly; `GET /state` has to tell them apart, because one is a robot
/// whose kernel has stopped and the other is every robot on this edition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelSighting {
    /// Nothing could be read, with the reason. Not a reading, and reported as unobserved.
    Unobservable(&'static str),
    /// The recording can hold a control cycle and none was written in this interval.
    Silent,
    /// Cycles were recorded: a control loop turned.
    Turning {
        /// How many, which is the interval's own evidence rather than a rate.
        cycles: u64,
    },
}

impl KernelSighting {
    /// The reading a tally implies, or its absence.
    ///
    /// Pure, so the rule can be tested without a disk — which matters more here than usual,
    /// because the interesting case is a ring **with** cycles in it and only a control loop can
    /// put one there (§26.2). A test that had to start one would be a rule nobody could check.
    pub fn from_tally(tally: Option<&skynet_blackbox::tally::Tally>) -> KernelSighting {
        let Some(tally) = tally else {
            return KernelSighting::Unobservable(
                "the ring could not be read, so nothing can be said about the control loop",
            );
        };
        if !tally.commands {
            return KernelSighting::Unobservable(
                "this recording has no action channels, so it cannot hold a control cycle — \
                 silence from it is not evidence about a kernel (§26, §74.3)",
            );
        }
        if tally.cycles == 0 {
            return KernelSighting::Silent;
        }
        KernelSighting::Turning {
            cycles: tally.cycles,
        }
    }

    /// §27's condition. `Live` for [`Turning`](KernelSighting::Turning) and nothing else.
    pub fn state(&self) -> skynet_contracts::opstate::KernelState {
        match self {
            KernelSighting::Turning { .. } => skynet_contracts::opstate::KernelState::Live,
            _ => skynet_contracts::opstate::KernelState::Down,
        }
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, KernelSighting::Unobservable(_))
    }

    /// What a console shows beside the condition.
    pub fn detail(&self) -> String {
        match self {
            KernelSighting::Unobservable(why) => (*why).to_string(),
            KernelSighting::Silent => {
                "no control cycle was recorded in the last interval, so nothing shows the loop \
                 turning. A hung kernel is an active unit, which is why this is read off the \
                 records rather than off systemd"
                    .to_string()
            }
            KernelSighting::Turning { cycles } => {
                format!("{cycles} control cycles were recorded in the last interval")
            }
        }
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
    let path = dir.join(&name);
    // Created new, never truncated — `skynet-kernel-host`'s writer's rule, and for its reason
    // (`DRY-COVERAGE-PLAN.md` F80): a sealed window is evidence, and `File::create` replaces
    // whatever held the name. The monotonic clock this is named by starts again at every boot.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| io_error("creating", &path, e))?;
    file.write_all(&sealed.bytes)
        .map_err(|e| io_error("writing", &path, e))?;
    file.sync_all()
        .map_err(|e| io_error("flushing", &path, e))?;
    // The digest, written by the party that computed it, at the moment it computed it. Best effort:
    // a window whose manifest could not be written is still a window, and §79's upload hashes one
    // that has none. Failing the seal for it would lose the recording to save a cache.
    let _ = write_manifest(dir, &name, sealed);
    Ok(path)
}

/// Where a sealed window's manifest lives.
///
/// Beside the recording and suffixed rather than substituted, so it is never mistaken for one:
/// `…-fault.mcap.window.json`. [`Recorder::sealed`] and §79's collector both filter on `.mcap`, so
/// the sidecar is invisible to everything that lists windows.
pub fn manifest_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.window.json"))
}

/// What a seal produced, written down at the seal (§77.1, §79).
///
/// **The digest is recorded by the producer, before any transfer exists** — which is §77.1's whole
/// argument for carrying it, applied one step earlier. The practical consequence is the one that
/// made this file necessary: §79's collector has to know which sealed windows are which, every
/// pass, and the only other way to answer that is to hash the directory. On a robot holding fifty
/// sealed windows that is gigabytes of reading per reconcile pass, to re-derive a number this
/// process already had in a variable.
///
/// It is a cache in the sense that it can be rebuilt, and it is **not** a second source of truth:
/// a manifest whose digest is not the file's digest is wrong, and §79's collector hashes rather
/// than believes one whenever it is about to act on it.
fn write_manifest(dir: &Path, name: &str, sealed: &Sealed) -> std::io::Result<()> {
    let mut document = Json::obj();
    document.set(
        "digest",
        Json::Str(format!(
            "sha256:{}",
            String::from_utf8_lossy(&skynet_sha256::hex_of(&sealed.digest))
        )),
    );
    document.set("size_bytes", Json::Num(sealed.bytes.len() as f64));
    document.set("records", Json::Num(sealed.continuity.records as f64));
    document.set("damaged", Json::Num(sealed.continuity.damaged as f64));
    document.set("reason", Json::Str(sealed.reason.as_str().to_string()));
    let text = document
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    crate::observe::write_atomic(&manifest_path(dir, name), text.as_bytes())
}

/// Read one back, when it is there and describes a file of this length.
///
/// The length is checked because a sealed window's *name* can repeat: §13 names it for a monotonic
/// instant, and a reboot restarts that clock. A manifest left beside a name that has since been
/// rewritten describes bytes that are gone, and returning its digest would be reporting a window
/// as collected because a previous one with the same name was.
///
/// The length is a cheap disqualifier and not a proof — two windows sealed off one ring are
/// frequently the same size — so §79's collector treats a surviving manifest as a hint and hashes
/// before it writes anything down. What the check buys is that the hint is almost never wrong,
/// which is what keeps the hash off the reconcile path.
pub fn read_manifest(dir: &Path, name: &str, size: u64) -> Option<String> {
    let document = crate::observe::read_json(&manifest_path(dir, name))?;
    let declared = document.get("size_bytes").and_then(Json::as_f64)?;
    if declared != size as f64 {
        return None;
    }
    document
        .get("digest")
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
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
