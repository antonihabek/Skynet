//! §13's ring, written by the process that sees every cycle.
//!
//! [`skynet_blackbox`] is the format and the arithmetic and opens nothing. `skynetd-io`'s recorder
//! is the half that opens files, and it writes **events and gaps** — cycles are this recorder's,
//! because the daemon has no control loop in it and §33.1 keeps it off that path.
//!
//! This is the recorder for the loop: the same format, the same preallocated ring, the same seal —
//! writing the channel the daemon never could.
//!
//! # Which of the two creates the ring, and why that is not this crate's decision
//!
//! Whichever starts first. On a bench that is this loop; on a robot it is `skynetd`, because §27
//! brings the supervisor up before anything it supervises. The creator sizes the ring, and a
//! creator that sized it for no body would leave the other writer unable to attach — which is what
//! made the daemon-first order the one that could not run a loop until `skynetd-io`'s
//! `shape::supervising` resolved a shape from the descriptor and the transport plan.
//!
//! The refusal below still exists and still matters: it is what a genuine disagreement about the
//! body looks like, rather than the ordinary consequence of booting.
//!
//! # One ring, one writer, and the lock that makes that true
//!
//! Two processes cannot write one ring today, and the reason is worth stating exactly rather than
//! working around quietly. [`skynet_blackbox::ring::Ring`] keeps its next sequence number **in
//! memory**. Two `Ring`s over one file would hand out the same sequence twice, write two records to
//! one slot, and produce a recording that looks intact and is not — which is the single worst
//! failure mode a black box has, because it is discovered by an investigator months later.
//!
//! So this takes an exclusive lock on the directory, and `skynetd`'s recorder must take the same
//! one. A second writer does not get a corrupted ring; it gets a refusal naming the process that
//! holds it. **Failing loudly beats corrupting evidence**, and that is the whole of the argument.
//!
//! The real fix is a ring whose sequence is an atomic in a mapped header, which is exactly what
//! `skynet-ring-shm` already does for §25's command ring. It is named as open in §26.2 rather than
//! half-built here, because a multi-writer ring wants a design and a Linux mapping, and the lock
//! makes the interim state safe rather than merely tolerable.
//!
//! # The shape is the body's, and a ring is recreated when the body changes
//!
//! A record's size is a function of the joint and channel counts, so a ring holds one shape. A robot
//! whose descriptor gains a joint gets its window **sealed and then replaced** — the records that
//! were there are still true, they are about a different shape of machine.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use skynet_blackbox::record::{Cycle, CycleView, Entry, EventCode, EventView, Shape};
use skynet_blackbox::ring::{Geometry, Meta, Ring};
use skynet_blackbox::seal::{seal, Reason, Sealed};
use skynet_lock::ring_file;
use skynet_lock::{Joining, Role, Writership};

use crate::motion::MotionTurn;

/// How many bytes the ring is preallocated with.
///
/// The same figure `skynetd-io` uses, and for the same reason: §13 asks for "roughly an hour" and
/// what an hour costs depends entirely on the body. At this size a two-joint arm at 200 Hz gets
/// hours; a 30-joint humanoid at 1 kHz gets about seventy seconds.
pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;

/// How many distinct policy digests the header table holds.
pub const DIGESTS: u16 = 16;

/// How many distinct capability tokens it holds.
pub const CAPABILITIES: u16 = 16;

/// Why a recorder could not be opened.
#[derive(Debug)]
pub enum RecorderError {
    /// The directory could not be joined at all (§26.2) — unwritable, in practice.
    Locked(skynet_lock::LockError),
    /// Another process created this recording and this one could not attach to it (§26.2).
    Attach(skynet_lock::AttachError),
    /// Another process created this recording with a ring shaped for a different body.
    Shape {
        /// Who holds the directory.
        holder: String,
        /// What the ring on disk records.
        found: Shape,
        /// What this loop records.
        wanted: Shape,
    },
    /// The directory could not be created, or the ring could not be written.
    Io(std::io::Error),
    /// The geometry the body implies is not one the format can hold.
    Geometry(String),
}

impl std::fmt::Display for RecorderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecorderError::Locked(error) => write!(f, "{error}"),
            RecorderError::Attach(error) => write!(f, "{error}"),
            RecorderError::Shape {
                holder,
                found,
                wanted,
            } => write!(
                f,
                "{holder} created this black box with a ring shaped for {} joints and {} channels, \
                 and this loop records {} and {}. A record's size is a function of those counts, so \
                 a cycle does not fit — and replacing the ring would delete the window the other \
                 writer is still filling. Point this loop at another directory, or give the other \
                 writer this body's shape.",
                found.joints, found.channels, wanted.joints, wanted.channels,
            ),
            RecorderError::Io(error) => write!(f, "{error}"),
            RecorderError::Geometry(detail) => write!(f, "sizing the black box: {detail}"),
        }
    }
}

impl std::error::Error for RecorderError {}

/// The ring, the file under it, and the lock that says this process owns it.
#[derive(Debug)]
pub struct Recorder {
    ring: Ring,
    path: PathBuf,
    sealed_dir: PathBuf,
    scratch: Vec<u8>,
    /// What this process has written that a ring's wrap must not take: §48.2's verdicts, newest per
    /// check, and §46.3's worst case. See [`Recorder::restate_post`].
    held: Held,
    // §26.2's writership. Dropped last of the fields it matters against, and only ever after the
    // file handles above are closed, because releasing the lock is what lets another writer create.
    writership: Writership,
}

/// The records a supervisor reads off the ring that are written once and not per cycle.
#[derive(Debug, Default)]
struct Held {
    /// (check, passed, evidence), in the order the checks were recorded.
    verdicts: Vec<(String, bool, String)>,
    /// The worst-case measurement and the cycles it stands over.
    timing: Option<(f64, u64)>,
    /// The sequence the held records were last written at, or the first of them.
    written_at: u64,
}

impl Recorder {
    /// Open the ring under `dir`, creating and preallocating it if absent or the wrong shape.
    pub fn open(
        dir: &Path,
        meta: Meta,
        shape: Shape,
        budget: usize,
    ) -> Result<Recorder, RecorderError> {
        fs::create_dir_all(dir).map_err(RecorderError::Io)?;
        let sealed_dir = dir.join("sealed");
        fs::create_dir_all(&sealed_dir).map_err(RecorderError::Io)?;

        // Before the ring, because a recorder that read the ring and then failed to join would have
        // touched a file another process owns.
        let joining = Writership::join(dir, "skynetk").map_err(RecorderError::Locked)?;

        let path = dir.join("ring");

        let creating = match joining {
            Joining::Creator(creating) => creating,
            // `skynetd` created this recording and is filling it with events. This loop attaches
            // and claims its sequences from the counter beside it — but unlike the daemon, it
            // cannot take any shape it finds: a cycle carries a value per joint and per channel, so
            // a ring shaped for a different body has no room for one.
            Joining::Attacher(attaching) => {
                let holder = attaching.holder().to_string();
                // Waited for, not read once. The writership named a live creator, so a ring that
                // is not on disk yet is one being written — see `skynet_lock::ring_file`, which
                // carries the run this was found on.
                let store = ring_file::read_when_published(&path, ring_file::PUBLISH_WAIT)
                    .map_err(RecorderError::Io)?;
                let ring = Ring::open(&store).map_err(|error| {
                    RecorderError::Geometry(format!(
                        "{holder} holds this directory and its ring cannot be opened here: {error:?}"
                    ))
                })?;
                if ring.geometry().shape != shape {
                    return Err(RecorderError::Shape {
                        holder,
                        found: ring.geometry().shape,
                        wanted: shape,
                    });
                }
                let writership = attaching.attach().map_err(RecorderError::Attach)?;
                let scratch = vec![0u8; ring.geometry().record_size];
                return Ok(Recorder {
                    ring,
                    path,
                    sealed_dir,
                    scratch,
                    held: Held::default(),
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
                        sealed_dir,
                        scratch,
                        held: Held::default(),
                        writership,
                    });
                }
                // The body changed under the recording. What is there is sealed before it is
                // replaced: those records are still true, they are about a different shape of robot.
                let closing = seal(&ring, &store, Reason::Shutdown, 0);
                let _ = write_sealed(&sealed_dir, &closing, 0);
            }
        }

        let geometry = Geometry::for_budget(shape, DIGESTS, CAPABILITIES, budget)
            .map_err(|error| RecorderError::Geometry(format!("{error:?}")))?;
        let (ring, store) = Ring::create(geometry, meta);

        // Preallocated by writing its whole length, zeroes included. A sparse file reports the same
        // size and fails at the first write after the disk fills, which is the failure §28 is
        // written against: a black box that stops recording because something else filled the disk
        // is a black box that fails exactly when something is going wrong.
        //
        // Published by rename. Writing those 64 MB at the final path leaves a file of every
        // intermediate length at a name the other writer is about to open, and §33.1 guarantees it
        // opens during exactly that window.
        ring_file::publish(&path, &store).map_err(RecorderError::Io)?;

        let scratch = vec![0u8; ring.geometry().record_size];
        let writership = creating.seed(ring.next_sequence());
        Ok(Recorder {
            ring,
            path,
            sealed_dir,
            scratch,
            held: Held::default(),
            writership,
        })
    }

    /// Which of §26.2's two roles this recorder has.
    pub fn role(&self) -> Role {
        self.writership.role()
    }

    /// The geometry this recording has.
    pub fn geometry(&self) -> Geometry {
        self.ring.geometry()
    }

    /// The sequence the next record will get.
    ///
    /// Off the shared counter when there is one: with two writers the ring's own is a floor rather
    /// than the answer, because it knows what this process staged and nothing about what the other
    /// has claimed since.
    pub fn next_sequence(&self) -> u64 {
        self.writership
            .peek()
            .unwrap_or_else(|| self.ring.next_sequence())
    }

    /// Append one control cycle.
    ///
    /// One positioned write of one record. No flush per cycle: at 200 Hz an `fsync` a cycle is five
    /// milliseconds of the budget spent proving the last five milliseconds happened, and §28's
    /// preallocation is what makes the write itself reliable. A power cut loses the page cache, and
    /// that is the trade §13 already makes by writing to a ring rather than to a journal.
    pub fn cycle(&mut self, mono_ns: u64, cycle: &Cycle) -> Result<u64, RecorderError> {
        // `Cycle` owns its vectors and `CycleView` borrows them, and the format takes the view.
        // Written out rather than derived, because a `From` that silently dropped a field the
        // format gained would be a recording missing something nobody looked for.
        let view = CycleView {
            tick: cycle.tick,
            observation: cycle.observation,
            policy: cycle.policy,
            state_flags: cycle.state_flags,
            fault_code: cycle.fault_code,
            rhal_result: cycle.rhal_result,
            // `Unrecorded` cannot be written: a record this build produces always says what the
            // read returned, and encoding "nothing is known" as a zero would put a schema-1 fact
            // in a schema-2 record. It is only ever the answer a *decoder* gives.
            read_result: match cycle.read {
                skynet_blackbox::record::ReadOutcome::Failed(code) => code,
                _ => 0,
            },
            state_seq: cycle.state_seq,
            state_stamp_ns: cycle.state_stamp_ns,
            command_stamp_ns: cycle.command_stamp_ns,
            command_seq: cycle.command_seq,
            mode: cycle.mode,
            advisory: cycle.advisory,
            response: cycle.response,
            tripped: &cycle.tripped,
            capability: cycle.capability,
            nearest_person_m: cycle.nearest_person_m,
            base_pose: cycle.base_pose,
            base_twist: cycle.base_twist,
            desired: &cycle.desired,
            command: &cycle.command,
            mask: &cycle.mask,
            clamped: &cycle.clamped,
            position: &cycle.position,
            velocity: &cycle.velocity,
            effort: &cycle.effort,
        };
        self.write(mono_ns, Entry::Cycle(&view))
    }

    /// Append §88.3's record of one motion request and what was decided about it.
    ///
    /// Every request and every verdict, accepted and refused alike, because §26's completeness
    /// argument is what makes an episode evidence: a recording that held only the motions which ran
    /// could not tell *"the skill asked for nothing"* from *"the skill was refused ten times"*.
    ///
    /// The subject is the motion's own name and the detail is the sentence the turn produced, which
    /// is the refusal's reason, the controller's reply, or §88.3's overdue bound. `skynetd` writes
    /// its own events onto the same ring under §26.2's claim, which is why this goes through the
    /// same `write` as a cycle does rather than opening anything of its own.
    pub fn motion(&mut self, mono_ns: u64, turn: &MotionTurn) -> Result<u64, RecorderError> {
        self.write(
            mono_ns,
            Entry::Event(&EventView {
                code: EventCode::MotionRequest,
                subject: &turn.motion,
                detail: &turn.detail,
            }),
        )
    }

    /// Append §89.2's record of a control-thread stream being suspended. Once per run per stream:
    /// [`crate::capture::Camera::settle`] answers the verdict only on the cycle that suspended it.
    pub fn capture_suspended(
        &mut self,
        mono_ns: u64,
        stream: &str,
        detail: &str,
    ) -> Result<u64, RecorderError> {
        self.write(
            mono_ns,
            Entry::Event(&EventView {
                code: EventCode::CaptureSuspended,
                subject: stream,
                detail,
            }),
        )
    }

    /// Append one §48.2 verdict from power-on self-test (§48.5).
    ///
    /// The subject is the check's name and the detail is its evidence in one sentence; the outcome
    /// is the code, so no reader parses a verdict out of prose. Written before the loop turns —
    /// and before this process exits on a failure — so the supervisor reads the verdict off the
    /// ring even though §33.1 forbids it running the check itself.
    pub fn post(
        &mut self,
        mono_ns: u64,
        check: &str,
        passed: bool,
        detail: &str,
    ) -> Result<u64, RecorderError> {
        let sequence = self.write_verdict(mono_ns, check, passed, detail)?;
        // Held so that [`Recorder::restate_post`] can write it again. A check recorded twice keeps
        // its newer verdict, the rule the supervisor reads them by.
        match self
            .held
            .verdicts
            .iter_mut()
            .find(|(held, _, _)| held == check)
        {
            Some(entry) => *entry = (check.to_string(), passed, detail.to_string()),
            None => self
                .held
                .verdicts
                .push((check.to_string(), passed, detail.to_string())),
        }
        self.held.written_at = sequence;
        Ok(sequence)
    }

    fn write_verdict(
        &mut self,
        mono_ns: u64,
        check: &str,
        passed: bool,
        detail: &str,
    ) -> Result<u64, RecorderError> {
        self.write(
            mono_ns,
            Entry::Event(&EventView {
                code: if passed {
                    EventCode::PostPassed
                } else {
                    EventCode::PostFailed
                },
                subject: check,
                detail,
            }),
        )
    }

    /// Write the verdicts and the worst case again once the ring has gone a quarter of the way
    /// round since they were last written (§48.5, §13).
    ///
    /// They are written once — the verdicts before the loop turns, the worst case when the warmup
    /// is out and then when it grows — and a control cycle is one record of a ring that holds a
    /// fixed number of them. The supervisor reads the verdicts off that ring on every pass and a
    /// verdict that has been written over reads as *unavailable*, which puts a robot that passed its
    /// self-test back in `post`: at 1 kHz and the daemon's default 64 MiB, four and a half minutes
    /// in (`DRY-COVERAGE-PLAN.md` F288). A quarter, so the newest of each is never more than a
    /// quarter of the ring old when it is read, and counted in the sequence the ring itself hands
    /// out, because the supervisor writes events into the same ring and a count of this process's
    /// own writes would run slow by exactly that much.
    ///
    /// This restates a result and does not re-run a check: the evidence is the sentence the check
    /// produced at start, unchanged, which is what a reader of a restated verdict is owed — and what
    /// §48.2's checks, run once because the checks that touch an actuator must not run again under a
    /// loop, can truthfully offer. A no-op until something has been held, and until the quarter.
    pub fn restate_post(&mut self, mono_ns: u64) -> Result<(), RecorderError> {
        if self.held.verdicts.is_empty() && self.held.timing.is_none() {
            return Ok(());
        }
        let quarter = (self.ring.geometry().capacity / 4).max(1) as u64;
        if self.next_sequence().saturating_sub(self.held.written_at) < quarter {
            return Ok(());
        }
        let verdicts = self.held.verdicts.clone();
        let mut last = self.held.written_at;
        for (check, passed, detail) in &verdicts {
            last = self.write_verdict(mono_ns, check, *passed, detail)?;
        }
        if let Some((worst_ms, cycles)) = self.held.timing {
            last = self.write_timing(mono_ns, worst_ms, cycles)?;
        }
        self.held.written_at = last;
        Ok(())
    }

    /// Append the kernel's worst-case cycle measurement (§46.3's first window).
    ///
    /// The subject is always `worst_case_cycle` and the detail is `worst <ms> ms over <cycles>
    /// cycles`, which the supervisor parses strictly when it checks the watchdog nesting. Called
    /// once after the warmup and then only when the worst grows — the newest event is the worst
    /// the loop has ever seen, and a measurement that never changed is not re-recorded.
    pub fn timing(
        &mut self,
        mono_ns: u64,
        worst_ms: f64,
        cycles: u64,
    ) -> Result<u64, RecorderError> {
        let sequence = self.write_timing(mono_ns, worst_ms, cycles)?;
        self.held.timing = Some((worst_ms, cycles));
        if self.held.written_at == 0 {
            self.held.written_at = sequence;
        }
        Ok(sequence)
    }

    fn write_timing(
        &mut self,
        mono_ns: u64,
        worst_ms: f64,
        cycles: u64,
    ) -> Result<u64, RecorderError> {
        let detail = format!("worst {worst_ms:.3} ms over {cycles} cycles");
        self.write(
            mono_ns,
            Entry::Event(&EventView {
                code: EventCode::KernelTiming,
                subject: "worst_case_cycle",
                detail: &detail,
            }),
        )
    }

    fn write(&mut self, mono_ns: u64, entry: Entry<'_>) -> Result<u64, RecorderError> {
        // §26.2. `None` is a lone creator on a platform with no shared mapping, where the ring's own
        // counter is correct because a second writer cannot exist.
        let (sequence, offset) = match self.writership.next() {
            Some(claimed) => {
                let offset = self
                    .ring
                    .stage_claimed(&mut self.scratch, mono_ns, entry, claimed)
                    .map_err(|error| {
                        RecorderError::Geometry(format!("encoding a record: {error:?}"))
                    })?;
                (claimed, offset)
            }
            None => self
                .ring
                .stage(&mut self.scratch, mono_ns, entry)
                .map_err(|error| {
                    RecorderError::Geometry(format!("encoding a record: {error:?}"))
                })?,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(RecorderError::Io)?;
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(RecorderError::Io)?;
        file.write_all(&self.scratch).map_err(RecorderError::Io)?;
        Ok(sequence)
    }

    /// Read the ring back, whole, because sealing produces the whole file.
    pub fn read(&self) -> Result<Vec<u8>, RecorderError> {
        let mut file = File::open(&self.path).map_err(RecorderError::Io)?;
        let mut store = Vec::new();
        file.read_to_end(&mut store).map_err(RecorderError::Io)?;
        Ok(store)
    }

    /// Seal the window into a file, and say what was written and where.
    ///
    /// The creator's, and only the creator's (§26.2): sealing reads the whole ring and writes it
    /// out under a digest, and two processes doing that produce two windows of one recording.
    ///
    /// # Two times, as the loop has two
    ///
    /// `mono_ns` is the kernel's time at the seal, on the clock the records are on — the
    /// simulation's, on a body that declares `simulator` (§35.3) — and it goes **into** the window,
    /// where it is compared with them. `named_ns` is this process's own clock, and it names the
    /// **file**: a simulation's time starts at zero every run, so two runs sealed into one directory
    /// at the same simulated instant would share a name, and a name is a fact about the machine the
    /// file is on. Until `DRY-COVERAGE-PLAN.md` F70 the two were one reading of the machine's clock,
    /// which is why nothing had needed them apart.
    ///
    /// A sealed window is evidence and is never replaced: a name that is taken anyway is refused.
    pub fn seal(
        &self,
        reason: Reason,
        mono_ns: u64,
        named_ns: u64,
    ) -> Result<(Sealed, PathBuf), RecorderError> {
        if !self.writership.may_seal() {
            return Err(RecorderError::Geometry(
                "this loop attached to a black box another process created, and the creator seals \
                 it (§26.2)"
                    .to_string(),
            ));
        }
        let store = self.read()?;
        let sealed = seal(&self.ring, &store, reason, mono_ns);
        let path = write_sealed(&self.sealed_dir, &sealed, named_ns).map_err(RecorderError::Io)?;
        Ok((sealed, path))
    }
}

/// Write a sealed window beside the ring, named by the instant it was sealed on this machine.
///
/// Created new, never truncated: `File::create` would replace a window already sealed under the
/// name, silently, and a sealed window is the one file this crate writes that is evidence.
fn write_sealed(dir: &Path, sealed: &Sealed, named_ns: u64) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // The same name `skynetd-io` writes, so one reader lists both directories the same way.
    let name = format!("{:020}-{}.mcap", named_ns, sealed.reason.as_str());
    let path = dir.join(name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "{} is already a sealed window, and a window is never overwritten — the \
                         one already there is kept",
                        path.display()
                    ),
                )
            } else {
                error
            }
        })?;
    file.write_all(&sealed.bytes)?;
    file.sync_all()?;
    Ok(path)
}
