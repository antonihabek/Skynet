//! §47's capture half, in the one process that holds the driver.
//!
//! `skynet-capture` decides who sees which frame and says nothing about where frames come from;
//! `skynet-frames` is the region a policy maps and says nothing about who fills it. This module is
//! what joins them to a body: a sensor read through the RHAL, the §47.4 split, and the pool.
//!
//! # Where a stream is read, and what that costs (§89)
//!
//! §47.2's fourth backend is *"anything only the driver can reach"*, and the driver is one C
//! context the control loop already holds on every cycle. A capture thread with its own reference to
//! that context is a second caller on an object whose thread-safety the ABI does not promise — unless
//! the driver does, which is what §89.1's `concurrent_read` is. So there are two placements:
//!
//! * **Undeclared, on the control thread.** A camera turn happens inside the cycle, after the
//!   command has been written — the write is the part with a deadline — and before the cycle is
//!   recorded. The read is timed, and [`Camera::settle`] holds it to §89.2 once the cycle's length
//!   is known: a stream whose reads keep costing the period is **suspended** for the run, so a slow
//!   sensor costs the policy its frames and never the kernel its period. `DRY-COVERAGE-PLAN.md` F24
//!   is the run that showed why — an 18 ms render on a 20 ms period, one overrun per frame.
//! * **Declared, on a [`CaptureThread`].** The camera moves to a thread of its own and reads through
//!   a [`ConcurrentSensor`], which `skynet-dl` makes only after reading the declaration out of the
//!   driver's own descriptor. The control loop never waits on it.
//!
//! # The recording side is declared empty
//!
//! §47.4 gives every frame to two consumers: the policy, latest-wins, and the black box, through a
//! bounded queue into a hardware encoder. **This build has no encoder.** A queue nothing drains fills
//! once and then refuses everything, which would be a gap that starts a few frames late and looks
//! like a stall. So the queue is declared at depth zero — `skynet-capture`'s own spelling of *"do not
//! record this stream"* — and every frame opens or extends one annotated gap. At the end of a run
//! [`Camera::close`] hands that gap back, and it says exactly what was not recorded.
//!
//! # What publishing allocates
//!
//! `skynet-frames` builds a `String` per slot on every publish, because §47.6's contract names a
//! stream by string and the pool reconciles every slot's loan against it. That is an allocation on
//! camera frames — a few tens a second — rather than on every cycle, and it is stated here rather
//! than assumed away: the loop's overrun count is what measures whether it matters on a machine.

use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use skynet_capture::source::{pump, Source};
use skynet_capture::{Captured, Distributor, Gap};
use skynet_contracts::media::{
    read_caused_miss, CapturePlacement, ControlThreadRead, Placement, ReadWindow,
};
use skynet_dl::driver::{ConcurrentSensor, SensorFrame};
use skynet_frames::{PoolError, Writer};

use crate::driver::{Actuator, ReadFailure};

/// What a sensor is read through: the loop's driver, or a capture thread's [`ConcurrentSensor`].
///
/// One method, so that one [`Camera`] runs on either thread and neither placement grows its own copy
/// of the read, the §47.4 split and the publish.
pub trait SensorPort {
    /// Read the named sensor's newest frame into `into`.
    fn read_sensor(&mut self, name: &CStr, into: &mut [u8]) -> Result<SensorFrame, ReadFailure>;
}

impl<A: Actuator + ?Sized> SensorPort for A {
    fn read_sensor(&mut self, name: &CStr, into: &mut [u8]) -> Result<SensorFrame, ReadFailure> {
        Actuator::read_sensor(self, name, into)
    }
}

impl SensorPort for ConcurrentSensor {
    fn read_sensor(&mut self, name: &CStr, into: &mut [u8]) -> Result<SensorFrame, ReadFailure> {
        // The handle was made for one sensor, because the declaration it rests on is per sensor. A
        // camera asking it for another is this crate's defect, and is refused rather than read.
        if name != self.name() {
            return Err(ReadFailure {
                code: -19,
                detail: format!(
                    "the capture thread holds a handle for `{}` and was asked for `{}` — a defect \
                     in skynet-kernel-host, not in the driver",
                    self.name().to_string_lossy(),
                    name.to_string_lossy()
                ),
            });
        }
        self.read(into)
            .map_err(|error| ReadFailure::from_driver(&error))
    }
}

/// One stream, as somebody declared it (§47.5).
///
/// Every number here is a declaration. A camera that renders on demand — MuJoCo's does — has no
/// frame rate that is a property of the body, so the rate is the operator's; and the buffer size is
/// the bound §47.5 asks for, never grown to fit a frame that did not.
#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    /// The sensor's name in the driver's descriptor, which is also the pool's stream name.
    pub name: String,
    /// How often to read it.
    pub hz: f64,
    /// How many frames this process may hold at once.
    pub buffers: usize,
    /// The largest frame, in bytes.
    pub slot_bytes: usize,
}

impl Declared {
    /// Check the declaration is one a stream can be built from, naming the fix when it is not.
    pub fn check(&self) -> Result<(), String> {
        if self.name.is_empty() || self.name.as_bytes().contains(&0) {
            return Err(format!(
                "`{}` is not a sensor name: the driver is asked for it as a C string, so it must be \
                 non-empty and contain no NUL byte",
                self.name
            ));
        }
        if !(self.hz.is_finite() && self.hz > 0.0) {
            return Err(format!(
                "a capture rate of {} Hz reads no frames. Declare the rate the stream should be \
                 read at (§47.5)",
                self.hz
            ));
        }
        if self.buffers == 0 || self.slot_bytes == 0 {
            return Err(
                "a stream needs at least one buffer of at least one byte; a stream of neither \
                 holds no frames"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// §47.2's fourth backend: frames only the driver can reach, read through `read_sensor`.
///
/// It owns its buffers — `buffers` of them, each `slot_bytes` long, allocated once — because the
/// ABI's frame is valid only until the next read of the same sensor, so a frame anybody will hold
/// past that has to be copied somewhere this process owns. Those copies are what [`Source`]'s
/// reference counting balances.
///
/// A read happens in [`RhalSensor::fill`], which is the device doing its work; [`Source::acquire`]
/// then hands over what it produced. The split is what lets this be a [`Source`] at all: the trait
/// takes no driver, and the driver is the loop's.
#[derive(Debug)]
pub struct RhalSensor {
    name: CString,
    stream: u16,
    period_ns: u64,
    next_ns: Option<u64>,
    seq: u64,
    buffers: Vec<Vec<u8>>,
    lens: Vec<usize>,
    refs: Vec<u8>,
    staged: Option<Captured>,
    last_stamp_ns: u64,
    counts: SensorCounts,
    first_failure: Option<ReadFailure>,
}

/// What a sensor did, for the life of the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SensorCounts {
    /// Reads attempted.
    pub reads: u64,
    /// Reads that produced a new frame.
    pub frames: u64,
    /// Reads the driver answered with the frame it had already given — the same acquisition stamp.
    ///
    /// Not a fault. Reading a stream faster than it produces is a configuration somebody chose,
    /// and a repeat is dropped here rather than handed on as a new frame, because a policy told the
    /// same image is fresh twice has been told something false about time.
    pub repeats: u64,
    /// Reads that failed.
    pub failures: u64,
    /// Frames that were due while every buffer was held, and so were never read (§47.5).
    ///
    /// The bounded-pool failure, counted rather than hidden. Nothing downstream can see these: a
    /// frame never read has no sequence number, so the only evidence it was due is this count.
    pub starved: u64,
}

impl RhalSensor {
    /// A sensor read at `declared.hz`, with `declared.buffers` buffers of `declared.slot_bytes`.
    ///
    /// `stream` is the index a [`Captured`] carries.
    pub fn new(stream: u16, declared: &Declared) -> Result<RhalSensor, String> {
        declared.check()?;
        let name = CString::new(declared.name.clone())
            .map_err(|_| format!("`{}` contains a NUL byte", declared.name))?;
        Ok(RhalSensor {
            name,
            stream,
            // Rounded down to whole nanoseconds, and never to zero: a period of zero would read
            // on every cycle, which is a rate nobody declared.
            period_ns: ((1e9 / declared.hz) as u64).max(1),
            next_ns: None,
            seq: 0,
            buffers: (0..declared.buffers)
                .map(|_| vec![0u8; declared.slot_bytes])
                .collect(),
            lens: vec![0; declared.buffers],
            refs: vec![0; declared.buffers],
            staged: None,
            last_stamp_ns: 0,
            counts: SensorCounts::default(),
            first_failure: None,
        })
    }

    /// The device's half of one turn: read a frame, if one is due and there is somewhere to put it.
    ///
    /// Due is decided by the declared rate on the loop's clock. A loop that has fallen more than a
    /// period behind reads **once** and does not try to catch up: a sensor read returns the newest
    /// frame, so a second read in the same turn could only return it again.
    pub fn fill<S: SensorPort + ?Sized>(&mut self, now_ns: u64, body: &mut S) {
        let due = *self.next_ns.get_or_insert(now_ns);
        if now_ns < due || self.staged.is_some() {
            return;
        }
        let next = due.saturating_add(self.period_ns);
        self.next_ns = Some(if next <= now_ns {
            now_ns.saturating_add(self.period_ns)
        } else {
            next
        });

        // The lowest free buffer, so the handle sequence is a property of the pool's state and a
        // test can assert on it.
        let Some(index) = self.refs.iter().position(|refs| *refs == 0) else {
            self.counts.starved += 1;
            return;
        };

        self.counts.reads += 1;
        match body.read_sensor(&self.name, &mut self.buffers[index]) {
            Ok(frame) if frame.stamp_ns <= self.last_stamp_ns => {
                self.counts.repeats += 1;
            }
            Ok(frame) => {
                self.counts.frames += 1;
                self.last_stamp_ns = frame.stamp_ns;
                self.lens[index] = frame.len;
                self.refs[index] = 1;
                self.staged = Some(Captured::device(
                    self.stream,
                    self.seq,
                    frame.stamp_ns,
                    index as u64,
                ));
                self.seq += 1;
            }
            Err(failure) => {
                self.counts.failures += 1;
                self.first_failure.get_or_insert(failure);
            }
        }
    }

    /// The bytes behind a frame this sensor lent out, while somebody still holds it.
    ///
    /// `None` for a handle this sensor did not issue or one that has been released — a released
    /// buffer is the next read's, and handing out its bytes would be handing out a frame that is
    /// about to become a different one.
    pub fn pixels(&self, frame: &Captured) -> Option<&[u8]> {
        let index = usize::try_from(frame.handle).ok()?;
        if frame.stream != self.stream || *self.refs.get(index)? == 0 {
            return None;
        }
        Some(&self.buffers[index][..self.lens[index]])
    }

    /// What the sensor has done so far.
    pub fn counts(&self) -> SensorCounts {
        self.counts
    }

    /// When the next read is due on the caller's clock, once the first has been made.
    pub fn next_due_ns(&self) -> Option<u64> {
        self.next_ns
    }

    /// The first read that failed, with the code the driver returned.
    ///
    /// The first rather than the last, because a stream that fails once and then keeps failing is
    /// described by the moment it started; a caller wanting the rate has [`SensorCounts::failures`].
    pub fn first_failure(&self) -> Option<&ReadFailure> {
        self.first_failure.as_ref()
    }
}

impl Source for RhalSensor {
    fn depth(&self) -> usize {
        self.buffers.len()
    }

    fn acquire(&mut self, _now_ns: u64) -> Option<Captured> {
        self.staged.take()
    }

    fn outstanding(&self) -> usize {
        self.refs.iter().filter(|refs| **refs > 0).count()
    }

    fn retain(&mut self, frame: Captured) {
        if let Some(refs) = self.refs.get_mut(frame.handle as usize) {
            *refs = refs.saturating_add(1);
        }
    }

    fn release(&mut self, frame: Captured) {
        if let Some(refs) = self.refs.get_mut(frame.handle as usize) {
            *refs = refs.saturating_sub(1);
        }
    }
}

/// What one camera turn did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureTurn {
    /// Frames taken from the sensor this turn — zero or one.
    pub captured: usize,
    /// The sequence published into the pool, when one was.
    pub published: Option<u64>,
    /// Why the frame this turn captured did not reach the pool, when it did not.
    pub refused: Option<String>,
}

/// What a camera did, for the life of the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CameraCounts {
    /// Frames published into the pool.
    pub published: u64,
    /// Frames §47.6 dropped because every slot was on loan (`pool_exhausted`).
    ///
    /// A policy holding every slot has denied itself vision, and this is how that is seen. Not a
    /// fault on this side: capture is never stalled, so the frame is counted and the next one goes.
    pub pool_dropped: u64,
    /// Frames the pool refused for any other reason — a frame larger than its slot, a mapping that
    /// failed.
    pub pool_failed: u64,
    /// Frames captured with nowhere to publish them, because no pool was offered.
    pub unpublished: u64,
    /// Turns that ended with every buffer held (§47.5).
    pub exhausted_turns: u64,
}

/// One declared stream, from the driver to the pool a policy maps.
pub struct Camera {
    sensor: RhalSensor,
    /// §47.4's split, with the recording side declared empty — see the module documentation.
    split: Distributor<0>,
    pool: Option<Writer>,
    counts: CameraCounts,
    first_refusal: Option<String>,
    /// Which thread reads it (§89). Only [`CaptureThread::spawn`] makes it anything but the control
    /// thread, because only a sensor the driver declared may be read anywhere else.
    placement: Placement,
    /// §89.2's window over this stream's control-thread reads.
    window: ReadWindow,
    /// What the reads are timed with.
    clock: fn() -> u64,
    /// How long this turn's read took, until [`Camera::settle`] takes it.
    read_ns: Option<u64>,
    /// The verdict that suspended the stream, once one has.
    suspended: Option<CapturePlacement>,
}

impl Camera {
    /// A camera over `sensor`, publishing into `pool` when there is one, read on the control thread.
    ///
    /// The control thread is where an undeclared sensor is read (§89.1), which is every sensor until
    /// [`CaptureThread::spawn`] is handed a [`ConcurrentSensor`] for it.
    pub fn new(sensor: RhalSensor, pool: Option<Writer>) -> Camera {
        Camera {
            sensor,
            split: Distributor::new(),
            pool,
            counts: CameraCounts::default(),
            first_refusal: None,
            placement: Placement::ControlThread,
            window: ReadWindow::default(),
            // §35.1's clock, the loop's own. Only differences are taken here, and one clock
            // across the process is still one fewer thing to reason about than two.
            clock: skynet_rt::monotonic_raw_ns,
            read_ns: None,
            suspended: None,
        }
    }

    /// The same camera, timing its reads with `clock` — for a test that needs a read to take a
    /// number of nanoseconds it can name.
    #[cfg(test)]
    pub fn with_clock(mut self, clock: fn() -> u64) -> Camera {
        self.clock = clock;
        self
    }

    /// Which thread reads this camera, or that nothing does any more (§89).
    pub fn placement(&self) -> Placement {
        if self.suspended.is_some() {
            Placement::Suspended
        } else {
            self.placement
        }
    }

    /// The verdict that suspended this stream, when one did (§89.2).
    pub fn suspension(&self) -> Option<CapturePlacement> {
        self.suspended
    }

    /// Hold this turn's read to §89.2, now that the cycle it happened in is over.
    ///
    /// `cycle_ns` is the whole cycle, read included, and `period_ns` the one it was meant to fit.
    /// Returns the verdict **once**, on the cycle that suspended the stream, so the caller prints it
    /// and records it exactly once; `None` on every other cycle, on a cycle with no read, and for a
    /// camera on a thread of its own — whose reads cost the control thread nothing.
    pub fn settle(&mut self, cycle_ns: u64, period_ns: u64) -> Option<CapturePlacement> {
        let read_ns = self.read_ns.take()?;
        if self.placement != Placement::ControlThread || self.suspended.is_some() {
            return None;
        }
        let read = ControlThreadRead {
            read_ns: read_ns as f64,
            rest_ns: cycle_ns.saturating_sub(read_ns) as f64,
        };
        let verdict = self.window.record(read_caused_miss(read, period_ns as f64));
        if verdict.placement == Placement::Suspended {
            self.suspended = Some(verdict);
            return Some(verdict);
        }
        None
    }

    /// The pool, for the hand-off that offers it to a policy.
    pub fn pool(&self) -> Option<&Writer> {
        self.pool.as_ref()
    }

    /// The pool's two descriptors, pixels then loans, for `handoff::Port::offer`.
    ///
    /// `None` without a pool, and on every platform but Linux: §47.6's mapping half is Linux's, and
    /// nowhere else has a policy to pass a pool to.
    #[cfg(target_os = "linux")]
    pub fn pool_descriptors(&self) -> Option<[std::ffi::c_int; 2]> {
        self.pool.as_ref().map(Writer::descriptors)
    }

    /// `None`: see the Linux build.
    #[cfg(not(target_os = "linux"))]
    pub fn pool_descriptors(&self) -> Option<[std::ffi::c_int; 2]> {
        None
    }

    /// Drop every loan in the pool, because every policy that held one has exited.
    ///
    /// `true` when there was a pool to do it to. The caller decides **when**, and there is exactly
    /// one condition under which it may: `handoff::Port::reap_departed`, which answers from a pidfd
    /// the kernel gave it. Calling this while a reader is alive takes a slot out from under it,
    /// which is the one property §47.6 exists to provide — so this deliberately reads nothing and
    /// decides nothing, and every word of the argument lives at the two ends that have the facts.
    pub fn revoke_pool_loans(&mut self) -> bool {
        match self.pool.as_mut() {
            Some(pool) => {
                pool.revoke_loans();
                true
            }
            None => false,
        }
    }

    /// The sensor's counts.
    pub fn sensor(&self) -> &RhalSensor {
        &self.sensor
    }

    /// The camera's counts.
    pub fn counts(&self) -> CameraCounts {
        self.counts
    }

    /// The first reason a frame did not reach the pool.
    pub fn first_refusal(&self) -> Option<&str> {
        self.first_refusal.as_deref()
    }

    /// One turn: read if due, hand the frame to both consumers, publish the freshest, release.
    ///
    /// Whatever the distributor hands back is released in the same turn, which is `pump`'s rule;
    /// the frame the policy's slot took is published and then released here, because the policy's
    /// real slot is the pool on the other side of the mapping, and holding a copy on this side too
    /// would be a buffer nobody reads.
    pub fn turn<S: SensorPort + ?Sized>(&mut self, now_ns: u64, body: &mut S) -> CaptureTurn {
        // Suspended is terminal for the run (§89.2): no further reads, and nothing to publish.
        if self.suspended.is_some() {
            return CaptureTurn::default();
        }
        let reads = self.sensor.counts.reads;
        let started = (self.clock)();
        self.sensor.fill(now_ns, body);
        if self.sensor.counts.reads > reads {
            self.read_ns = Some((self.clock)().saturating_sub(started));
        }
        let pumped = pump(&mut self.sensor, &mut self.split, now_ns, 1);
        if pumped.source_exhausted {
            self.counts.exhausted_turns += 1;
        }

        let mut turn = CaptureTurn {
            captured: pumped.captured,
            ..CaptureTurn::default()
        };
        let Some(frame) = self.split.latest.take() else {
            return turn;
        };

        match self.pool.as_mut() {
            None => self.counts.unpublished += 1,
            Some(pool) => {
                // A frame the slot holds always has its bytes: the slot's reference is what keeps
                // the buffer from being reused. `None` would be this module's own bug, and it is
                // refused by name rather than published as an empty frame a policy would decode.
                let published = match self.sensor.pixels(&frame) {
                    Some(pixels) => pool.publish(frame.seq, frame.stamp_ns, pixels),
                    None => Err(PoolError::Geometry(
                        "the frame's buffer was released before it was published — a defect in \
                         skynet-kernel-host's capture turn, not in the driver",
                    )),
                };
                match published {
                    Ok(_) => {
                        self.counts.published += 1;
                        turn.published = Some(frame.seq);
                    }
                    Err(error) => {
                        if matches!(error, PoolError::Refused(_)) {
                            self.counts.pool_dropped += 1;
                        } else {
                            self.counts.pool_failed += 1;
                        }
                        let said = error.to_string();
                        self.first_refusal.get_or_insert_with(|| said.clone());
                        turn.refused = Some(said);
                    }
                }
            }
        }
        self.sensor.release(frame);
        turn
    }

    /// The end of a run: close the recording side's gap and hand it back.
    ///
    /// `None` only when no frame was ever captured — on this build every captured frame is in it.
    pub fn close(&mut self) -> Option<Gap> {
        self.split.record.close_gap()
    }
}

/// A declared stream, read on a thread of its own (§89.3).
///
/// Owns the [`Camera`] — its buffers, its §47.4 split and its pool — for as long as the thread runs,
/// and hands it back at [`CaptureThread::join`] for the end-of-run report. The loop keeps what it
/// needs without reaching in: the pool's two descriptors, taken before the camera moved, which is
/// all `handoff::Port::offer` ever reads; and a request flag for revoking loans, which the thread
/// acts on at its next turn. Neither ever makes the control loop wait.
pub struct CaptureThread {
    stream: String,
    descriptors: Option<[std::ffi::c_int; 2]>,
    has_pool: bool,
    stop: Arc<AtomicBool>,
    revoke: Arc<AtomicBool>,
    handle: Option<JoinHandle<Camera>>,
}

/// How long the capture thread sleeps at most between turns, so that a stop is honoured promptly
/// even for a stream read once a second.
const CAPTURE_IDLE: Duration = Duration::from_millis(5);

impl CaptureThread {
    /// Move `camera` onto a thread that reads it through `sensor`, at the camera's declared rate.
    ///
    /// `sensor` is the proof that the driver declared the stream `concurrent_read`: `skynet-dl` will
    /// not make one otherwise. The thread reads the loop's clock — §35.1's, absolute — so a frame's
    /// `now` means what it means to the control loop.
    pub fn spawn(
        mut camera: Camera,
        mut sensor: ConcurrentSensor,
    ) -> Result<CaptureThread, String> {
        camera.placement = Placement::OwnThread;
        let stream = sensor.name().to_string_lossy().into_owned();
        let descriptors = camera.pool_descriptors();
        let has_pool = camera.pool.is_some();
        let stop = Arc::new(AtomicBool::new(false));
        let revoke = Arc::new(AtomicBool::new(false));
        let (thread_stop, thread_revoke) = (Arc::clone(&stop), Arc::clone(&revoke));
        let handle = std::thread::Builder::new()
            .name(format!("capture {stream}"))
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    if thread_revoke.swap(false, Ordering::AcqRel) {
                        camera.revoke_pool_loans();
                    }
                    let now_ns = skynet_rt::monotonic_raw_ns();
                    camera.turn(now_ns, &mut sensor);
                    // Until the next frame is due, and never longer than `CAPTURE_IDLE`.
                    let after = skynet_rt::monotonic_raw_ns();
                    let wait = camera
                        .sensor
                        .next_due_ns()
                        .map_or(CAPTURE_IDLE, |due| {
                            Duration::from_nanos(due.saturating_sub(after))
                        })
                        .min(CAPTURE_IDLE);
                    std::thread::sleep(wait);
                }
                camera
            })
            .map_err(|error| {
                format!("the capture thread for `{stream}` could not start: {error}")
            })?;
        Ok(CaptureThread {
            stream,
            descriptors,
            has_pool,
            stop,
            revoke,
            handle: Some(handle),
        })
    }

    /// The stream this thread reads.
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The pool's two descriptors, for the hand-off that offers it to a policy.
    pub fn pool_descriptors(&self) -> Option<[std::ffi::c_int; 2]> {
        self.descriptors
    }

    /// Ask the thread to drop every loan in the pool, for [`Camera::revoke_pool_loans`]'s reason and
    /// only on its condition. Acted on at the thread's next turn; `true` when there is a pool.
    pub fn request_revoke(&self) -> bool {
        if !self.has_pool {
            return false;
        }
        self.revoke.store(true, Ordering::Release);
        true
    }

    /// Stop the thread and take the camera back, for the end-of-run report.
    ///
    /// Joined before the driver is dropped — `main` holds this for exactly that reason — so the
    /// context a [`ConcurrentSensor`] reads through is never closed on the capture thread.
    pub fn join(mut self) -> Result<Camera, String> {
        self.stop.store(true, Ordering::Release);
        match self.handle.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| format!("the capture thread for `{}` panicked", self.stream)),
            None => Err(format!(
                "the capture thread for `{}` was already joined",
                self.stream
            )),
        }
    }
}
