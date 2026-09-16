//! §47's capture half, in the one process that holds the driver.
//!
//! `skynet-capture` decides who sees which frame and says nothing about where frames come from;
//! `skynet-frames` is the region a policy maps and says nothing about who fills it. This module is
//! what joins them to a body: a sensor read through the RHAL, the §47.4 split, and the pool.
//!
//! # Why the loop does it, and what that costs
//!
//! §47.2's fourth backend is *"anything only the driver can reach"*, and the driver is one C
//! context the control loop already holds mutably on every cycle. A capture thread with its own
//! reference to that context would be a second caller on an object whose thread-safety the ABI never
//! promised. So a camera turn happens **inside** the cycle, after the command has been written — the
//! write is the part with a deadline — and before the cycle is recorded.
//!
//! That puts a sensor read on the control thread, and the cost is not hidden: a frame is read only
//! when one is due at the stream's declared rate, a read that takes longer than the period is an
//! overrun the loop already counts, and a driver whose render is slow shows up there rather than as
//! a mystery. §23.3's rule is about *inference*, which this is not; but the same instinct applies,
//! and the answer here is to measure the read rather than pretend it is free.
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

use std::ffi::CString;

use skynet_capture::source::{pump, Source};
use skynet_capture::{Captured, Distributor, Gap};
use skynet_frames::{PoolError, Writer};

use crate::driver::{Actuator, ReadFailure};

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
    pub fn fill<A: Actuator>(&mut self, now_ns: u64, body: &mut A) {
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
}

impl Camera {
    /// A camera over `sensor`, publishing into `pool` when there is one.
    pub fn new(sensor: RhalSensor, pool: Option<Writer>) -> Camera {
        Camera {
            sensor,
            split: Distributor::new(),
            pool,
            counts: CameraCounts::default(),
            first_refusal: None,
        }
    }

    /// The pool, for the hand-off that offers it to a policy.
    pub fn pool(&self) -> Option<&Writer> {
        self.pool.as_ref()
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
    pub fn turn<A: Actuator>(&mut self, now_ns: u64, body: &mut A) -> CaptureTurn {
        self.sensor.fill(now_ns, body);
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
