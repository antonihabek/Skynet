//! What a policy is, from the ring's side of the sandbox.
//!
//! One trait, three intents and two backends. Everything here is pure — no clock of its own, no
//! ring, no I/O — so the cases worth testing are reachable from a test rather than from a robot.

use skynet_obs::Observation;
use skynet_ring::{mode, MAX_CHANNELS};

/// What a policy is permitted to command, fixed for the life of a lease.
///
/// Not advice and not a default: [`crate::run::PolicyHost`] refuses an [`Intent`] that exceeds it,
/// by name, and the refusal is a fault rather than a clamp. Clamping belongs to the kernel, where
/// §7's guards are, and a runtime that quietly narrowed a command would be a second opinion about
/// safety in the one process that has no business having one.
///
/// # `channels` is §26.3's missing number
///
/// §80 records the daemon's ring-shape question as blocked on a channel count, *"because channels
/// are the policy runtime's and that runtime is unwritten"*. This is the field it was waiting for.
/// A policy declares how many channels it commands when its lease is granted, which is before
/// either ring is opened, so a supervisor sizing a recording for the body has something true to
/// read rather than a guess.
// No `Eq`: the budget is an `f64`. `PartialEq` is enough for the one thing this is compared for,
// which is a test asserting a lease was carried through unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Commanding {
    /// How many channels this policy may write. Never more than [`MAX_CHANNELS`].
    pub channels: usize,
    /// How old a body reading may be before this policy may not use it, in milliseconds.
    ///
    /// §25.1: *"A staleness budget per stream, from the policy's declared observation requirements.
    /// Exceeding it is a `hold`, not a silently older frame."* The runtime enforces it rather than
    /// each backend, because it is a safety-relevant default and a backend that forgot it would be
    /// a policy acting on a body that has moved since.
    ///
    /// `None` means the policy declared no budget, which — per §25.1's own note on the field — is
    /// **not** a licence to use any sample at all. It means nothing here can say whether the
    /// reading is good enough, so the reading is passed through and the turn counts it as
    /// unbudgeted. A closed-loop policy that declares no budget is a policy nobody can hold to one.
    pub observation_budget_ms: Option<f64>,
    /// How old a camera frame may be before this policy may not use it, in milliseconds.
    ///
    /// §25.1's staleness budget is *per stream*, and a camera is a stream with its own latency:
    /// thirty milliseconds is old for a joint reading and ordinary for an image. So it is its own
    /// number rather than the body's, and exceeding it is a hold the backend never sees, for the
    /// same reason. `None` passes every frame, and the backend is told each one's age.
    pub frame_budget_ms: Option<f64>,
    /// The control mode resolved for this package by §6's transport plan.
    ///
    /// Fixed for the lease. A backend returning a different one is refused rather than followed:
    /// a policy that switched from position to torque between cycles would be commanding a joint
    /// in units the plan was not resolved against, and the ceilings the kernel is clamping to were
    /// computed for the mode in the plan.
    pub mode: u32,
}

impl Commanding {
    /// A policy that commands nothing, in position mode.
    ///
    /// The honest default, and it is not a useful one: `channels: 0` means every [`Intent`] this
    /// runtime can accept is [`Intent::Hold`] or [`Intent::Silent`]. It exists so that a
    /// `Commanding` somebody forgot to fill in commands no joint rather than the first one.
    pub const fn none() -> Commanding {
        Commanding {
            channels: 0,
            observation_budget_ms: None,
            frame_budget_ms: None,
            mode: mode::POSITION,
        }
    }

    /// Whether `channels` and `mode` are ones this ring can carry at all.
    pub fn valid(&self) -> bool {
        self.channels <= MAX_CHANNELS
            && matches!(self.mode, mode::POSITION | mode::VELOCITY | mode::TORQUE)
    }
}

/// What a policy wants this cycle.
///
/// The three-way split is the point; see the table in the crate documentation. `Hold` and `Silent`
/// are different events on a robot and an `Option<Command>` would have spelled them the same.
// Clippy asks for the large variant to be boxed. It must not be: this type is produced once per
// control cycle on the path §25 draws, and a `Box` there is an allocation per cycle in a loop whose
// whole property is a deadline it makes every time. The size is 640 bytes of fixed arrays, which is
// what `MAX_CHANNELS` costs and is the same shape the ring slot it is written into already has —
// `Intent` is a value on the stack, never a collection this process keeps.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intent {
    /// Command these channels, in the mode the lease resolved.
    Command {
        /// The commanded values, in the units the lease's mode implies. Only the first
        /// [`Commanding::channels`] entries are read.
        values: [f32; MAX_CHANNELS],
        /// Per channel: zero holds the joint at its current setpoint, non-zero applies the value.
        mask: [u8; MAX_CHANNELS],
    },
    /// Alive, and commanding nothing.
    ///
    /// Published as a command whose mask is entirely zero and whose sequence advances, so the
    /// watchdog sees a producer that is running and asking for nothing. On a torque-controlled
    /// joint a zero mask holds position; a zero *value* would go limp, which is §24's reason for
    /// the mask existing at all.
    Hold,
    /// Nothing is published and the sequence does not advance.
    ///
    /// The kernel then reads the previous slot again, its watchdog guard sees a sequence that has
    /// not moved, and the robot stops in a controlled way. That is the correct outcome: a policy
    /// that has stopped producing is a fault, and the response to it lives in the kernel rather
    /// than here.
    Silent,
}

impl Intent {
    /// A command over `values`, applying every one of them.
    ///
    /// The convenience a backend that computes a full setpoint vector wants, and it is deliberately
    /// not the only way to build an [`Intent::Command`]: a policy that wants to move three joints
    /// of a twelve-joint body must be able to say so, which is what a mask is for.
    pub fn apply(values: &[f32]) -> Intent {
        let mut command = [0.0; MAX_CHANNELS];
        let mut mask = [0u8; MAX_CHANNELS];
        for (index, value) in values.iter().take(MAX_CHANNELS).enumerate() {
            command[index] = *value;
            mask[index] = 1;
        }
        Intent::Command {
            values: command,
            mask,
        }
    }
}

/// What the runtime could tell a backend about the robot's body this cycle.
///
/// The four cases are four different situations and none of them is an error. Collapsing them into
/// `Option<&Observation>` would lose the one distinction that matters most on a robot: whether a
/// policy is open-loop **by configuration** or open-loop because the path that should be feeding it
/// has stopped.
#[derive(Debug, Clone, Copy)]
pub enum Body<'a> {
    /// A reading, within its budget if the lease declared one.
    Fresh {
        /// The body, as the process holding the RHAL last published it.
        observation: &'a Observation,
        /// How old the reading is at this tick, in milliseconds.
        ///
        /// Computed from [`Observation::stamp_ns`], which §24 requires to be the instant of
        /// **acquisition**. An age measured from a publish stamp would be the number this runtime
        /// wishes were true.
        age_ms: f64,
        /// Whether the lease declared a budget this was checked against.
        ///
        /// `false` is the §25.1 case where nothing can say the reading is good enough. A backend
        /// that cares may refuse to act on it; the runtime passes it and counts it.
        budgeted: bool,
    },
    /// An observation ring is attached and nothing has ever been published to it.
    ///
    /// Normal at start: a policy that came up before the process holding the driver. Not a fault,
    /// and §25.3's answer for a policy that cannot decide is a hold.
    Empty,
    /// An observation ring is attached and the producer overwrote the slot mid-read, repeatedly.
    Torn,
    /// No observation ring is attached at all.
    ///
    /// An **open-loop deployment**, which is a real configuration — a homing sequence and a
    /// calibration sweep need no body — and is emphatically not the same as a closed-loop policy
    /// whose feed has died. That one is [`Body::Empty`] or a stale hold the backend never sees.
    Detached,
}

/// What the runtime could show a backend of the robot's camera this cycle (§47.6).
///
/// Four cases for [`Body`]'s reason: each sends a person somewhere different, and an `Option` would
/// make *no camera on this robot* and *the camera has not delivered yet* the same value.
#[derive(Debug, Clone, Copy)]
pub enum Sight<'a> {
    /// The newest frame, copied out of the pool under a loan that was released before the backend
    /// was asked.
    ///
    /// Copied because §47.6 lets the writer reuse a slot the moment it is released, and a backend
    /// that took several milliseconds over a borrow into the mapping would be holding the slot for
    /// all of them — which is what loans are for, and what this runtime chooses not to spend on a
    /// policy that has not asked for it.
    Frame {
        /// The pixels, as the capture side published them.
        pixels: &'a [u8],
        /// The capture side's sequence for this frame. A backend that sees it jump has missed
        /// frames, which is ordinary: the pool is latest-wins by design (§47.4).
        sequence: u64,
        /// Acquisition, on the same clock as the body's stamp (§47.2).
        stamp_ns: u64,
        /// How old the frame is at this tick, in milliseconds, measured from acquisition.
        age_ms: f64,
    },
    /// A pool is attached and nothing has been published into it yet — §47.6's sentinel, which is
    /// what keeps this from being a frame of zeros.
    NoFrameYet,
    /// The newest frame was replaced between being named and being taken.
    ///
    /// Not a fault: the writer outran this reader, and §47.6 refuses the stale handle by name
    /// rather than showing somebody else's frame. The next tick names the new one.
    Missed,
    /// No pool is attached. A robot with no camera, which is most of them.
    Blind,
}

/// A policy, as the ring sees one.
///
/// Deliberately narrow. A backend receives the tick it is answering and the monotonic instant that
/// tick began, and returns what it wants. It is handed no ring, no driver and no clock of its own,
/// so the only thing it can do is decide — which is what makes every backend in this file testable
/// without a robot, and what would make a neural network backend testable without a GPU.
///
/// # What a backend can see
///
/// [`Body`] carries proprioception, which is what `skynet-obs` carries, which is what fits in a
/// page. [`Sight`] carries the camera: §47.6's pool, filled since 2026-09-16 by the control loop
/// reading the driver's sensor. Until that producer existed this trait had no frame parameter, on
/// the argument that a parameter nothing can fill is a worse lie than a missing one — and the
/// argument still holds for a robot with no camera, which is why [`Sight::Blind`] is a case and not
/// an empty slice.
pub trait Backend {
    /// A short name for logs and the black box. Never a claim about what the policy does.
    fn name(&self) -> &'static str;

    /// Decide what to command on this tick.
    ///
    /// `tick` counts from zero and advances once per control period whatever this returns, so a
    /// backend can tell "the twentieth cycle" from "the twentieth command".
    ///
    /// `body` is what the runtime could learn about the robot. A backend that ignores it is
    /// open-loop, which is a legitimate thing to be and is what two backends in this file are.
    ///
    /// `sight` is the newest camera frame, when there is a camera and it has delivered one.
    fn decide(&mut self, tick: u64, now_ns: u64, body: Body<'_>, sight: Sight<'_>) -> Intent;
}

/// The policy that commands nothing, for ever.
///
/// Not a placeholder and not a stub — it is a real and useful policy. A robot with a lease granted
/// and no behaviour loaded should hold position with a live producer, which is exactly this: the
/// watchdog stays satisfied, the guards evaluate a real command every cycle, and the robot does
/// not move. It is what a policy runtime should be running while an operator decides what to run.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hold;

impl Backend for Hold {
    fn name(&self) -> &'static str {
        "hold"
    }

    fn decide(&mut self, _tick: u64, _now_ns: u64, _body: Body<'_>, _sight: Sight<'_>) -> Intent {
        Intent::Hold
    }
}

/// A table of setpoints, played once, open-loop.
///
/// # Open-loop, and the word is doing work
///
/// It reads nothing about the robot. It does not know whether the previous setpoint was reached,
/// refused by a guard, or clamped; it advances on the clock and commands the next row regardless.
/// That is a genuine and limited kind of policy — it is what a homing sequence and a calibration
/// sweep are — and it is emphatically not what a manipulation policy is.
///
/// It ships because it is the smallest backend that produces a **changing** command, which is what
/// the five telemetry counters, the guard-trip column and the field-conformance ratio have all been
/// waiting for. A robot running this is a robot whose ring has action channels.
///
/// # What it does when the table runs out
///
/// [`Intent::Hold`], for ever, and not [`Intent::Silent`]. A trajectory that has finished is a
/// policy that has finished successfully and is still there; stopping the sequence would present it
/// to the watchdog as a crashed producer and stop the robot with a fault. The distinction is the
/// crate's whole design and this is the first place it decides something.
#[derive(Debug, Clone)]
pub struct Trajectory {
    rows: Vec<(u64, [f32; MAX_CHANNELS], [u8; MAX_CHANNELS])>,
    next: usize,
}

impl Trajectory {
    /// Build a trajectory from `(tick, values)` rows, applying every value in each row.
    ///
    /// Rows are played in the order given, one per tick at or after the tick it names. They are not
    /// sorted for the caller: a table out of order is a table somebody built wrongly, and silently
    /// reordering setpoints is how a sweep runs backwards on one robot and forwards on another.
    pub fn from_setpoints(rows: &[(u64, &[f32])]) -> Trajectory {
        let mut built = Vec::with_capacity(rows.len());
        for (at, values) in rows {
            let mut command = [0.0; MAX_CHANNELS];
            let mut mask = [0u8; MAX_CHANNELS];
            for (index, value) in values.iter().take(MAX_CHANNELS).enumerate() {
                command[index] = *value;
                mask[index] = 1;
            }
            built.push((*at, command, mask));
        }
        Trajectory {
            rows: built,
            next: 0,
        }
    }

    /// Whether every row has been played.
    pub fn finished(&self) -> bool {
        self.next >= self.rows.len()
    }
}

impl Backend for Trajectory {
    fn name(&self) -> &'static str {
        "trajectory"
    }

    fn decide(&mut self, tick: u64, _now_ns: u64, _body: Body<'_>, _sight: Sight<'_>) -> Intent {
        match self.rows.get(self.next) {
            Some((at, values, mask)) if *at <= tick => {
                let intent = Intent::Command {
                    values: *values,
                    mask: *mask,
                };
                self.next += 1;
                intent
            }
            // Either the table is exhausted, or the next row is not due yet. Both are a live policy
            // asking for nothing on this cycle, and neither is a stalled producer.
            _ => Intent::Hold,
        }
    }
}

/// A proportional move toward a fixed target, closed on the body.
///
/// The smallest backend that genuinely reads what it is given: each cycle it takes the measured
/// joint positions, computes `position + gain * (target - position)`, and commands that. With no
/// body it commands nothing, which is the entire point of it existing beside [`Trajectory`].
///
/// # What it is for
///
/// It is not a control design. A proportional step with no velocity term, no feed-forward and no
/// model is a way to walk a joint toward a number, and the reason it ships is that it is the
/// smallest thing that closes the loop — the first backend whose command on cycle *n* depends on
/// what the robot did on cycle *n − 1*. Everything §25.1 exists for becomes observable through it:
/// a stale reading changes what is commanded, and a body that is not moving is visible in the
/// command sequence rather than only in the black box.
///
/// # It refuses to guess
///
/// A joint whose measured position is `NaN` — the value `skynet-obs` writes above a declared joint
/// count, and the one a driver reports for a quantity it does not measure — is **masked out**
/// rather than commanded from a guessed position. Commanding `NaN * gain` would put a `NaN` on the
/// wire; commanding from an assumed zero would drive a joint to its origin because nothing measured
/// it. Masking it out holds it, which is §24's meaning of a zero mask and the only honest answer.
#[derive(Debug, Clone)]
pub struct Approach {
    target: [f32; MAX_CHANNELS],
    channels: usize,
    gain: f32,
}

impl Approach {
    /// Move toward `target`, closing `gain` of the remaining distance each cycle.
    ///
    /// `gain` is clamped into `(0, 1]`: zero would never arrive and above one would overshoot every
    /// cycle, and both are configuration mistakes rather than control choices worth honouring.
    pub fn toward(target: &[f32], gain: f32) -> Approach {
        let mut goal = [0.0; MAX_CHANNELS];
        for (index, value) in target.iter().take(MAX_CHANNELS).enumerate() {
            goal[index] = *value;
        }
        Approach {
            target: goal,
            channels: target.len().min(MAX_CHANNELS),
            gain: if gain.is_finite() {
                gain.clamp(f32::EPSILON, 1.0)
            } else {
                f32::EPSILON
            },
        }
    }
}

/// §23.3's in-process inference backend, as a [`Backend`].
///
/// The decision — what reaches the model, what is refused, what an output becomes — lives in
/// `skynet-infer`, one crate below this one, and this is the adapter that turns [`Body`] and
/// [`Sight`] into its arguments. The layering is not decoration: a session that links a real
/// inference library has to be selectable by `skynetp`, which is a binary **in this crate**, so a
/// trait declared here could never be implemented by a crate that binary depends on.
///
/// # It is the first backend here that is a function of a frame
///
/// [`Trajectory`] ignores the body and [`Approach`] ignores the camera. This one reads both, and it
/// is therefore the first thing in the workspace for which §47's whole path — a driver's
/// `read_sensor`, §47.4's split, §47.6's pool, the loan, the copy — changes what a joint is told to
/// do. Until it existed the pool was filled and read and nothing downstream depended on a pixel.
///
/// # Every hold is a hold, and the reason is counted rather than published
///
/// A refusal inside the policy becomes [`Intent::Hold`]: a live producer commanding nothing, which is
/// §25.3's answer and keeps the watchdog satisfied while the robot does not move. Which of the seven
/// reasons it was is in `skynet_infer::Counts`, because a `Tick` is what the ring saw and a policy's
/// reason for holding is not something the ring can carry.
#[derive(Debug)]
pub struct Inference<S: skynet_infer::Session> {
    policy: skynet_infer::Policy<S>,
}

impl<S: skynet_infer::Session> Inference<S> {
    /// Run `policy` as a backend.
    pub fn over(policy: skynet_infer::Policy<S>) -> Inference<S> {
        Inference { policy }
    }

    /// What the policy did, cycle by cycle and reason by reason.
    pub fn counts(&self) -> skynet_infer::Counts {
        self.policy.counts()
    }

    /// The first thing the session refused, if it ever did.
    pub fn first_refusal(&self) -> Option<&str> {
        self.policy.first_refusal()
    }

    /// What the runner and the artefact are.
    pub fn describe(&self) -> &str {
        self.policy.describe()
    }
}

impl<S: skynet_infer::Session> Backend for Inference<S> {
    fn name(&self) -> &'static str {
        "inference"
    }

    fn decide(&mut self, _tick: u64, _now_ns: u64, body: Body<'_>, sight: Sight<'_>) -> Intent {
        // `joints` is what the driver declares it measures, and everything above it in the arrays is
        // `NaN` by §24. Slicing to it is what lets the policy answer *the body is narrower than this
        // model* rather than *joint 3 is unmeasured*, which are different things to go and look at.
        let reading = match body {
            Body::Fresh { observation, .. } => {
                let joints = observation.joints.min(observation.position.len());
                Some(skynet_infer::Reading {
                    positions: &observation.position[..joints],
                    velocities: &observation.velocity[..joints],
                    efforts: &observation.effort[..joints],
                })
            }
            Body::Empty | Body::Torn | Body::Detached => None,
        };
        let pixels = match sight {
            Sight::Frame { pixels, .. } => Some(pixels),
            // A frame the writer moved is not a frame, and neither is no camera. The runtime already
            // counts which of the three it was, and re-deriving it here would be a second record of
            // a fact that can only disagree with the first.
            Sight::NoFrameYet | Sight::Missed | Sight::Blind => None,
        };

        match self.policy.decide(reading, pixels) {
            skynet_infer::Decision::Command(values) => Intent::apply(values),
            skynet_infer::Decision::Hold(_) => Intent::Hold,
        }
    }
}

impl Backend for Approach {
    fn name(&self) -> &'static str {
        "approach"
    }

    fn decide(&mut self, _tick: u64, _now_ns: u64, body: Body<'_>, _sight: Sight<'_>) -> Intent {
        let Body::Fresh { observation, .. } = body else {
            // No body, for any of the three reasons. A closed-loop policy with nothing to close on
            // is a policy that should not be commanding, and a hold says so while staying alive.
            return Intent::Hold;
        };

        let mut values = [0.0; MAX_CHANNELS];
        let mut mask = [0u8; MAX_CHANNELS];
        let width = self.channels.min(observation.joints);
        for joint in 0..width {
            let measured = observation.position[joint];
            if !measured.is_finite() {
                // Masked out: held where it is, rather than commanded from a position nobody read.
                continue;
            }
            values[joint] = measured + self.gain * (self.target[joint] - measured);
            mask[joint] = 1;
        }
        Intent::Command { values, mask }
    }
}
