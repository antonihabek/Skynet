//! The loop.
//!
//! Read the body, take whatever the policy has posted, ask the kernel, write what it allows, record
//! what happened. Once per control period, for as long as the robot is on.
//!
//! # The three rules that shape it
//!
//! **It never blocks on the producer** (§25). A cycle takes whatever is in the ring at the instant
//! it looks — [`Reading::Fresh`], [`Reading::Empty`] or [`Reading::Stale`] — and none of the three
//! is a reason to wait. A kernel that waited for a policy would have the policy's tail latency as
//! its real-time guarantee, which is no guarantee.
//!
//! **A cycle always produces a record.** Including the cycles where nothing was posted, the driver
//! refused the command, or the read failed. §15's `field_conformance` is a ratio over cycles, and a
//! loop that recorded only the interesting ones would compute it over a denominator that flattered
//! the robot. The record is the cycle, not the event.
//!
//! **`last_sequence` is what the loop remembers, and almost nothing else.** The kernel holds no
//! state between cycles, so the small amount there has to be — the last sequence the watchdog
//! compares against, what was actually sent for the rate limiter, and where a ramp has got to —
//! lives in [`Host`] and is passed back in. Keeping it here rather than in the kernel is what lets
//! the kernel stay `no_std` with no allocator.
//!
//! # Where the tier comes from, and where it does not
//!
//! Nothing in this file decides a tier. [`skynet_kernel::Tier::enforcement`] derives enforcement
//! from the tier and no caller may pass one in, and the tier arrives on the resolved limits that
//! were computed when the lease was granted. Running this loop is not what makes a robot host-tier:
//! that is a property of whether this process is scheduled before the next control deadline, which
//! is a fact about the kernel configuration underneath it. [`Summary::overruns`] counts the cycles
//! where it demonstrably was not, which is the honest thing this loop can contribute to that
//! question.

use skynet_blackbox::Cycle;
use skynet_kernel::{Envelope, ResolvedLimits, MAX_CHANNELS};
use skynet_ring::{Consumer, Reading};

use crate::capture::{Camera, CaptureTurn};
use crate::cycle::{decide, record, Decision, Observed, Posted, Stopping};
use crate::driver::Actuator;

/// A monotonic clock, so a test can drive the loop without waiting.
///
/// §35.1: control reads one clock and it is monotonic. The trait exists because a loop that called
/// `Instant::now` directly could only be tested in real time, and a ramp that takes forty cycles
/// would take forty control periods to test.
pub trait Clock {
    /// Nanoseconds on a monotonic base. The origin does not matter; only differences do.
    fn now_ns(&mut self) -> u64;
}

/// `CLOCK_MONOTONIC_RAW` where the platform has it, and the closest available elsewhere.
#[derive(Debug)]
pub struct SystemClock {
    origin: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock {
            origin: std::time::Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now_ns(&mut self) -> u64 {
        self.origin.elapsed().as_nanos() as u64
    }
}

/// A clock a test advances by hand.
#[derive(Debug, Default)]
pub struct StepClock {
    /// The current reading.
    pub now_ns: u64,
    /// How far [`Clock::now_ns`] advances it on each call.
    pub step_ns: u64,
}

impl Clock for StepClock {
    fn now_ns(&mut self) -> u64 {
        let now = self.now_ns;
        self.now_ns += self.step_ns;
        now
    }
}

/// What one turn of the loop did.
#[derive(Debug, Clone)]
pub struct Turn {
    /// The record for this cycle.
    pub cycle: Cycle,
    /// What the kernel decided.
    pub decision: Decision,
    /// What the ring had.
    pub reading: Reading,
    /// Whether the driver's state read failed, with the code the record keeps and the sentence a
    /// person reads.
    pub read_error: Option<crate::driver::ReadFailure>,
    /// Whether an emergency stop was demanded and the body has none.
    pub stop_unavailable: bool,
    /// What the camera did, when one is attached.
    pub capture: Option<CaptureTurn>,
}

/// What a cycle publishes to, beyond the command it writes to the body.
///
/// Both are optional and both are the loop's to feed rather than to own: the mappings belong to
/// whoever created them, and a cycle borrows them for as long as it takes to write into them.
#[derive(Default)]
pub struct Attached<'a, 'p> {
    /// §25's observation ring: the body this cycle read, for a policy in another process.
    pub observations: Option<&'a mut skynet_obs::Producer<'p>>,
    /// §47's capture: a declared stream, read through the RHAL and published into the pool.
    pub camera: Option<&'a mut Camera>,
}

impl<'a, 'p> Attached<'a, 'p> {
    /// Nothing attached: the body is driven and nothing else hears about it.
    pub fn nothing() -> Attached<'a, 'p> {
        Attached::default()
    }
}

/// Counters worth carrying out of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Cycles turned.
    pub cycles: u64,
    /// Cycles where the kernel had to intervene — any guard tripped (§15).
    pub interventions: u64,
    /// Cycles where the ring had nothing new.
    pub idle: u64,
    /// Cycles where the driver's state read failed.
    pub read_failures: u64,
    /// Cycles where `write_command` returned non-zero.
    pub write_failures: u64,
    /// Emergency stops performed.
    pub stops: u64,
    /// Cycles that took longer than the control period.
    ///
    /// The one number this loop can honestly contribute to the question of whether a robot is
    /// really at host tier. It counts a deadline that was demonstrably missed; it does not
    /// establish that the others were met.
    pub overruns: u64,
}

/// The loop's memory.
///
/// Everything the kernel deliberately does not keep. One per body.
pub struct Host<'a> {
    channels: &'a [&'a str],
    tick: u64,
    last_sequence: Option<u64>,
    stopping: Stopping,
    /// What was actually sent last cycle, which the rate-limit guard compares against.
    previous: Vec<(&'a str, f64)>,
    /// Reused every cycle so the hot path allocates nothing after the first.
    desired: Vec<(&'a str, f64)>,
    /// This cycle's measured state, copied off the driver so the write can borrow it back.
    state_position: Vec<f32>,
    state_velocity: Vec<f32>,
    state_effort: Vec<f32>,
    frame: skynet_ring::Command,
    /// One reusable observation, for the mirror ring. Filled and published each cycle when a
    /// producer is attached; never allocated in the hot path, like everything else here.
    body_out: skynet_obs::Observation,
    have_command: bool,
    summary: Summary,
}

impl<'a> Host<'a> {
    /// A host for a body with these channels, in transport-plan order.
    pub fn new(channels: &'a [&'a str]) -> Host<'a> {
        Host {
            channels,
            tick: 0,
            last_sequence: None,
            stopping: Stopping::No,
            previous: Vec::with_capacity(channels.len()),
            desired: Vec::with_capacity(channels.len()),
            state_position: Vec::with_capacity(channels.len()),
            state_velocity: Vec::with_capacity(channels.len()),
            state_effort: Vec::with_capacity(channels.len()),
            body_out: skynet_obs::Observation::default(),
            frame: skynet_ring::Command::default(),
            have_command: false,
            summary: Summary::default(),
        }
    }

    /// The counters so far.
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Turn the loop once, with nothing reading the body.
    ///
    /// Equivalent to [`Host::step_observing`] with no producer: the robot is driven and the state
    /// it reports goes nowhere but the black box. That is the deployment every robot had before
    /// `skynet-obs` existed, and it is still the right one for an open-loop policy.
    pub fn step<A: Actuator>(
        &mut self,
        now_ns: u64,
        body: &mut A,
        ring: &mut Consumer<'_>,
        limits: &ResolvedLimits<'a>,
        envelope: Option<&Envelope<'a>>,
    ) -> Turn {
        self.step_observing(now_ns, body, ring, limits, envelope, Attached::nothing())
    }

    /// Turn the loop once, publishing the body this cycle read and whatever the camera captured.
    ///
    /// # Why this process is the one that publishes it
    ///
    /// It is already holding the answer. §7's guards need the measured state every cycle, so the
    /// read has happened and the arrays are in hand; publishing is a copy into a mapping this
    /// process opened. The alternative — the policy runtime opening its own handle on the driver —
    /// is the thing §25 forbids outright (*"cannot reach the RHAL at all"*) and would put a second
    /// reader on a device that mostly does not have one.
    ///
    /// # It is published whatever the cycle did
    ///
    /// Including cycles where the read **failed**. A failed read leaves the previous arrays in
    /// place and `sample` as `None`, so what goes out carries the fallback stamp and a zero
    /// sequence — and a policy watching the sequence stop advancing learns exactly what the black
    /// box learns. Skipping the publish would leave the policy reading a stale slot with no way to
    /// tell it apart from a body that is not moving.
    ///
    /// # The camera turns after the write
    ///
    /// The command is the part of a cycle with a deadline, so it goes to the body first; a frame
    /// read afterwards costs the time left in the period, and a read that costs more than that is an
    /// overrun the caller counts. See [`crate::capture`] for why the read is on this thread at all.
    pub fn step_observing<A: Actuator>(
        &mut self,
        now_ns: u64,
        body: &mut A,
        ring: &mut Consumer<'_>,
        limits: &ResolvedLimits<'a>,
        envelope: Option<&Envelope<'a>>,
        attached: Attached<'_, '_>,
    ) -> Turn {
        let Attached {
            observations,
            camera,
        } = attached;
        self.tick += 1;
        self.summary.cycles += 1;
        let now_ms = now_ns as f64 / 1_000_000.0;

        // ── Read the body ────────────────────────────────────────────────────
        //
        // A failed read does not skip the cycle. The kernel still runs, on the command that is
        // there, and the record says the state was not read — a loop that returned early here would
        // leave a gap in a recording with no entry explaining it.
        let (sample, read_error) = match body.sample(now_ns) {
            Ok(sample) => (Some(sample), None),
            Err(error) => {
                self.summary.read_failures += 1;
                (None, Some(error))
            }
        };

        // The three arrays are copied into buffers this loop owns rather than borrowed from the
        // driver, because the same cycle both reads the body and writes to it, and a borrow of the
        // driver's arrays would still be live at the write. They are taken out of `self` and put
        // back at the end so the capacity is reused: the copy is a `memcpy` of a few hundred bytes,
        // not an allocation, which is what §24's "reused across cycles" asks for.
        let mut position = std::mem::take(&mut self.state_position);
        let mut velocity = std::mem::take(&mut self.state_velocity);
        let mut effort = std::mem::take(&mut self.state_effort);
        {
            let (from_position, from_velocity, from_effort) = body.state();
            position.clear();
            position.extend_from_slice(from_position);
            velocity.clear();
            velocity.extend_from_slice(from_velocity);
            effort.clear();
            effort.extend_from_slice(from_effort);
        }

        let observed = Observed {
            stamp_ns: sample.map_or(now_ns, |sample| sample.stamp_ns),
            seq: sample.map_or(0, |sample| sample.seq),
            flags: sample.map_or(0, |sample| sample.flags),
            fault_code: sample.map_or(0, |sample| sample.fault_code),
            position: &position,
            velocity: &velocity,
            effort: &effort,
            base_pose: sample.map_or([f32::NAN; 7], |sample| sample.base_pose),
            base_twist: sample.map_or([0.0; 6], |sample| sample.base_twist),
        };

        // ── Publish the body, if anybody is listening ────────────────────────
        if let Some(producer) = observations {
            let joints = position.len().min(skynet_obs::MAX_JOINTS);
            self.body_out.stamp_ns = observed.stamp_ns;
            self.body_out.seq = observed.seq as u64;
            self.body_out.joints = joints;
            self.body_out.flags = observed.flags as u32;
            self.body_out.fault_code = observed.fault_code;
            self.body_out.base_pose = observed.base_pose;
            self.body_out.base_twist = observed.base_twist;
            for (joint, measured) in position.iter().take(joints).enumerate() {
                // `NaN` where the driver reported fewer entries than positions, which is the same
                // value `skynet-obs` writes above a declared joint count and means the same thing:
                // there is no measurement here. A zero would be a claim.
                self.body_out.position[joint] = *measured;
                self.body_out.velocity[joint] = velocity.get(joint).copied().unwrap_or(f32::NAN);
                self.body_out.effort[joint] = effort.get(joint).copied().unwrap_or(f32::NAN);
            }
            // The error is unreachable — `joints` is clamped to the ceiling above — and it is
            // discarded rather than unwrapped because a panic in the control loop over a mirror
            // ring would stop a robot that is otherwise running correctly. The black box is the
            // record that matters, and it is written either way.
            let _ = producer.publish(&self.body_out);
        }

        // ── Take whatever the policy posted ──────────────────────────────────
        let reading = ring.read(&mut self.frame);
        match reading {
            Reading::Fresh => self.have_command = true,
            // Empty is a policy that has not spoken; Stale is one publishing faster than this loop
            // can copy a slot. Neither waits, and the second runs on the command already held —
            // which is what `have_command` is for.
            Reading::Empty | Reading::Stale => {
                if !self.have_command {
                    self.summary.idle += 1;
                }
            }
        }

        let mut desired = std::mem::take(&mut self.desired);
        desired.clear();
        if self.have_command {
            for (index, channel) in self.channels.iter().take(MAX_CHANNELS).enumerate() {
                // A channel the policy masked out is one it is **not** commanding, so it is absent
                // from `desired` rather than present with a value. Putting it in would have the
                // envelope guard check a number the policy never asked for.
                if index < self.frame.channels && self.frame.mask[index] != 0 {
                    desired.push((*channel, self.frame.values[index] as f64));
                }
            }
        }

        let posted = self.have_command.then_some(Posted {
            desired: &desired,
            command_ms: self.frame.stamp_ns as f64 / 1_000_000.0,
            sequence: self.frame.seq,
            mode: self.frame.mode,
            // This edition's ring carries no capability token. §7.2's rule is that a guard with no
            // input reports unavailable rather than passing, and `None` is what produces that — it
            // is not the same as saying the command exercises no capability.
            capability: None,
        });

        // ── Ask the kernel ───────────────────────────────────────────────────
        let previous = (!self.previous.is_empty()).then_some(self.previous.as_slice());
        let decision = decide(
            now_ms,
            &observed,
            posted.as_ref(),
            self.channels,
            limits,
            envelope,
            previous,
            self.last_sequence,
            self.stopping,
            // The pose guards need a frame graph and a joint binding, which are the transport
            // plan's. Until a plan reaches this loop they are reported unavailable by
            // `resolve_limits`, which is the honest state rather than a body invented here.
            None,
        );
        self.stopping = decision.stopping;

        // ── Write what it allowed ────────────────────────────────────────────
        let width = decision.channels;
        let rhal_result = body.command(
            self.frame.stamp_ns,
            self.frame.seq as u32,
            decision.mode as u8,
            &decision.values[..width],
            &decision.mask[..width],
            now_ns,
        );
        if rhal_result != 0 {
            self.summary.write_failures += 1;
        }

        let mut stop_unavailable = false;
        if decision.cut_power {
            if body.has_stop() {
                if body.stop().is_ok() {
                    self.summary.stops += 1;
                }
            } else {
                // A body with no emergency stop cannot perform Cat 0, and the loop does not pretend
                // otherwise. The command it has already written is a full hold mask, which is the
                // strongest thing available here; the caller is told, because a robot that cannot
                // cut power is a fact for a person rather than a counter.
                stop_unavailable = true;
            }
        }

        // ── Capture, if a stream is declared ─────────────────────────────────
        //
        // The body's clock rather than a second reading of this one: `now_ns` is the instant this
        // cycle began, and a stream's rate is judged against cycles.
        let capture = camera.map(|camera| camera.turn(now_ns, body));

        // ── Remember, for the next cycle ─────────────────────────────────────
        self.previous.clear();
        for (index, channel) in self.channels.iter().take(width).enumerate() {
            if decision.mask[index] != 0 {
                self.previous
                    .push((*channel, decision.values[index] as f64));
            }
        }
        if let Some(posted) = posted.as_ref() {
            self.last_sequence = Some(posted.sequence);
        }

        let cycle = record(
            self.tick,
            // §26 records the hash of the observation frame the policy was given. No frame is
            // assembled on this edition — that is the policy runtime's half of §25.1 — so this is
            // zero, which the format's readers already treat as "no frame", rather than a hash of
            // something this loop made up.
            [0u8; 32],
            0,
            skynet_blackbox::record::NO_CAPABILITY,
            self.channels,
            &observed,
            posted.as_ref(),
            &decision,
            rhal_result,
            read_error.as_ref().map_or(0, |failure| failure.code),
        );
        if cycle.intervened() {
            self.summary.interventions += 1;
        }

        // The four buffers go back with their capacity, which is the whole reason they were taken
        // rather than allocated. Everything borrowing them is dead by here — `record` returned an
        // owned `Cycle`, and `posted` is not used again.
        self.desired = desired;
        self.state_position = position;
        self.state_velocity = velocity;
        self.state_effort = effort;

        Turn {
            cycle,
            decision,
            reading,
            read_error,
            stop_unavailable,
            capture,
        }
    }

    /// Note that a cycle overran its period.
    pub fn note_overrun(&mut self) {
        self.summary.overruns += 1;
    }
}
