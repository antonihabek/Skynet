//! One control cycle, decided.
//!
//! This module is the crank that §25 draws and nothing had ever turned. The pieces either side of
//! it have existed for a long time: [`skynet_ring`] is the channel a policy posts into,
//! [`skynet_kernel::evaluate_cycle_with_body`] is the eight guards, [`skynet_rhal`] is the wire to
//! the motors, and [`skynet_blackbox`] is the record. What did not exist is the function that reads
//! one command off the ring, asks the kernel what may be sent, and produces the bytes for both the
//! driver and the recorder.
//!
//! It is pure, and that is the whole reason it is its own module. It takes no clock, opens no
//! library, touches no ring and writes no file — `now_ms` arrives as a parameter for §35.2's reason,
//! and everything it decides is a function of its arguments. The loop in [`crate::run`] is what has
//! the I/O in it, and the loop is the part that cannot be tested against a thousand cases.
//!
//! # Three translations, and each one is a place this could be quietly wrong
//!
//! ## A hold is a mask of zeros, never a command of zeros
//!
//! [`skynet_kernel::CycleVerdict::command`] is **empty** under a hold or a stop — unless the
//! verdict is advisory, and that exception is the whole of the second rule below. The RHAL wire has
//! no way to say "nothing": it has `values` and a `mask`, and §24 defines a zero mask entry as
//! *hold this joint at its current setpoint*. So a hold is written as a full command frame whose
//! mask is all zeros, and the values are irrelevant.
//!
//! Sending zeros with a mask of ones instead would be the worst bug this file could contain. On a
//! position-controlled arm it commands the zero pose — every joint slams to its origin. On a
//! torque-controlled one it commands zero torque, which is not holding but going limp. §24 spells
//! out that difference and this is the code where it is either honoured or not.
//!
//! Not writing at all is a third thing, and also wrong: the driver's own `on_command_loss` timeout
//! would eventually fire, which is a *different* response from the one the guard chose, arriving at
//! a time nobody selected.
//!
//! ## An advisory verdict is not enforced, and that includes by this module
//!
//! [`skynet_kernel::Tier::enforcement`] derives *advisory* from a tier that cannot promise a clamp,
//! and `evaluate_cycle` already encodes what that means: under `advisory` the verdict's command is
//! the **desired** command rather than the clamped one, so that a guest-tier deployment does not
//! look enforced in its own logs.
//!
//! Everything this module does after reading the verdict — withholding a mask, cutting power,
//! starting a ramp — is enforcement, and on an advisory verdict none of it runs. Getting this wrong
//! is not a small bug: a robot would withhold a command the kernel had said to forward, while the
//! record beside it said `advisory: true`. It **was** wrong here for an afternoon, and what found it
//! was `package.grants.gate` in the other repository half — a conformance test whose expectations
//! are written against the kernel rather than against this file.
//!
//! What still happens on an advisory verdict is the recording. The guards' trips and the demanded
//! response are carried into the black box unchanged, because observing and reporting is the whole
//! of what the guest tier does.
//!
//! ## The deceleration bound is the acceleration bound
//!
//! §7.1's ladder has two responses this edition has to construct rather than forward:
//! [`FaultResponse::StopCat2`] is *"controlled ramp to zero velocity"* and
//! [`FaultResponse::StopCat1`] is that ramp followed by power removal. A ramp needs a rate, and
//! **no deceleration figure exists anywhere** — not in [`Envelope`], not in
//! [`skynet_kernel::ResolvedLimits`], not in the descriptor. Inventing one would be inventing a
//! safety number, which is the one thing this codebase does not do.
//!
//! It does not have to be invented, because a declared one already means it.
//! [`ResolvedLimits::max_delta_per_cycle`] is the jerk ceiling the rate-limit guard already enforces
//! on the way **up**: the largest fraction of a channel's ceiling the machine tolerates changing in
//! one cycle. A ramp down at that same rate is inside what the robot is declared to withstand, by
//! the same declaration. It is not a new number and it is not a guess; it is the existing number
//! read in the other direction, and that is stated here so nobody later reads the ramp as an
//! invention.
//!
//! When `max_delta_per_cycle` is absent there is no rate at all, and the ramp cannot be performed as
//! specified. It is **escalated to [`FaultResponse::StopCat0`]** — power removed immediately — and
//! [`Decision::escalated`] records that it happened and that the missing rate is why. Escalating is
//! the wrong trade for a humanoid holding a crate, and doing it silently would be worse than doing
//! it: an operator reading a black box needs to see that the robot performed a harder stop than the
//! guard asked for, and the reason, or they will conclude the guard demanded it.
//!
//! ## A stop is a state, not an instant
//!
//! Cat 1 and Cat 2 are not decided in one cycle — the ramp runs across many, and power is removed at
//! the end of it rather than at the start. [`Stopping`] is that state, carried by the caller between
//! cycles, because the kernel holds nothing and this module allocates nothing.

use skynet_blackbox::Cycle;
use skynet_kernel::{
    evaluate_cycle_with_body, Body, CommandCycle, Envelope, FaultResponse, Guard, ResolvedLimits,
    MAX_CHANNELS,
};

/// What was read off the robot this cycle.
///
/// A borrowed view rather than a copy: the loop owns the arrays the driver filled, and this crosses
/// into the kernel without any of them being moved.
#[derive(Debug, Clone, Copy)]
pub struct Observed<'a> {
    /// `rhal_state.stamp_ns`, at acquisition (§24).
    pub stamp_ns: u64,
    /// `rhal_state.seq`.
    pub seq: u32,
    /// `rhal_state.flags`.
    pub flags: u16,
    /// `rhal_state.fault_code`, surfaced verbatim.
    pub fault_code: u32,
    /// Measured joint positions, one per channel of the transport plan, in plan order.
    pub position: &'a [f32],
    /// Measured velocities; `NaN` where the joint does not measure it (§24).
    pub velocity: &'a [f32],
    /// Measured efforts; `NaN` where unmeasured.
    pub effort: &'a [f32],
    /// `x y z qx qy qz qw`, or all `NaN` when the base is not localised.
    pub base_pose: [f32; 7],
    /// `vx vy vz wx wy wz`.
    pub base_twist: [f32; 6],
}

/// What the policy posted, as it came off the ring.
///
/// Passing `None` for a whole `Posted` is a robot whose policy has not produced a command — normal
/// at start, and not a fault. The watchdog guard is what decides whether an *old* command is a
/// fault, and it needs the stamp to do it, so nothing is filtered here.
///
/// Two lifetimes, and the split is load-bearing. `'a` is the **channel names**, which come from the
/// transport plan and outlive every cycle. `'d` is the **slice**, which the loop rebuilds each cycle
/// into a buffer it owns. Collapsing them into one would force the plan's lifetime down to a single
/// cycle's and make the loop allocate a fresh list every time round.
#[derive(Debug, Clone, Copy)]
pub struct Posted<'d, 'a> {
    /// The desired values, paired with the channel names of the transport plan.
    pub desired: &'d [(&'a str, f64)],
    /// When the policy produced it (§24), in milliseconds on the kernel's monotonic base.
    pub command_ms: f64,
    /// The command sequence.
    pub sequence: u64,
    /// One of [`skynet_ring::mode`].
    pub mode: u32,
    /// The capability token the runtime said this command exercises, when it said.
    ///
    /// `None` is *the runtime did not say*, which §7.2 treats differently from "no capability": the
    /// mode gate reports unavailable rather than passing.
    pub capability: Option<&'a str>,
}

/// Where a multi-cycle stop has got to.
///
/// Carried by the caller across cycles. Cat 0 never appears here — it is performed in the cycle it
/// is decided and there is nothing left to be in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stopping {
    /// No stop in progress.
    #[default]
    No,
    /// Ramping toward zero; power is retained afterwards (§7.1 Cat 2).
    RampingToHold,
    /// Ramping toward zero; power is removed when it arrives (§7.1 Cat 1).
    RampingToOff,
}

/// What the kernel decided to put on the wire, and what to record.
#[derive(Debug, Clone)]
pub struct Decision {
    /// The values to write, one per channel of the transport plan, in plan order.
    ///
    /// Meaningless where [`Decision::mask`] is zero, and deliberately left as whatever the ramp or
    /// the clamp produced rather than zeroed: a reader of the black box can see what the kernel was
    /// aiming at even on a cycle it sent nothing.
    pub values: [f32; MAX_CHANNELS],
    /// Per channel: 0 means hold this joint, non-zero means apply (§24).
    pub mask: [u8; MAX_CHANNELS],
    /// How many entries of `values` and `mask` carry meaning.
    pub channels: usize,
    /// The mode to write, which is the posted mode unless a ramp changed it.
    pub mode: u32,
    /// Whether the driver's `emergency_stop` is to be called this cycle.
    pub cut_power: bool,
    /// The stop state to carry into the next cycle.
    pub stopping: Stopping,
    /// The strongest response any guard demanded.
    pub response: Option<FaultResponse>,
    /// Guards that tripped, in evaluation order.
    pub tripped: [Option<Guard>; 8],
    /// The response actually performed, when it is not the one demanded, and why.
    ///
    /// `Some` only when the kernel did something **harder** than it was asked to, which happens for
    /// exactly one reason: a ramp was demanded and no rate exists to ramp at.
    pub escalated: Option<Escalation>,
    /// Whether the command was forwarded unchanged despite a trip, because the tier cannot enforce.
    pub advisory: bool,
    /// Per channel: whether the kernel reduced this one.
    ///
    /// Per channel rather than a count, because that is what the black box records — a count would
    /// say a clamp happened and not which joint was clamped, and the joint is the whole question
    /// when somebody is reading a recording months later.
    pub clamped: [bool; MAX_CHANNELS],
}

/// A response performed in place of the one demanded, with the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Escalation {
    /// What §7.1's ladder asked for.
    pub demanded: FaultResponse,
    /// What this edition could actually perform.
    pub performed: FaultResponse,
    /// Why, in the words an operator reads off the black box.
    pub because: &'static str,
}

/// The rate a ramp runs at, when one is declared.
///
/// A channel's step is its own ceiling times the jerk fraction, so a fast joint ramps faster than a
/// slow one in absolute terms and both take the same number of cycles. That is the same arithmetic
/// [`Guard::RateLimit`] applies on the way up, which is the point.
fn ramp_step(limits: &ResolvedLimits<'_>, channel: &str) -> Option<f64> {
    let fraction = limits.max_delta_per_cycle?;
    let ceiling = limits.ceiling_for(channel)?;
    let step = (ceiling * fraction).abs();
    if step.is_finite() && step > 0.0 {
        Some(step)
    } else {
        None
    }
}

/// Move one value toward zero by at most `step`.
fn toward_zero(value: f64, step: f64) -> f64 {
    if value.abs() <= step {
        0.0
    } else if value > 0.0 {
        value - step
    } else {
        value + step
    }
}

/// Decide one cycle.
///
/// `previous` is what was actually sent last cycle, which the rate-limit guard needs and which the
/// caller holds because the kernel holds nothing.
///
/// Ten parameters, and they are not bundled into a struct on purpose: every one of them is an
/// *input to a safety decision*, and a struct with a `Default` would let a caller omit one and get a
/// cycle evaluated against a value nobody supplied. `evaluate_cycle_with_body` in the kernel takes
/// its inputs the same way, for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn decide<'a>(
    now_ms: f64,
    observed: &Observed<'_>,
    posted: Option<&Posted<'_, 'a>>,
    channels: &[&'a str],
    limits: &ResolvedLimits<'a>,
    envelope: Option<&Envelope<'a>>,
    previous: Option<&[(&'a str, f64)]>,
    last_sequence: Option<u64>,
    stopping: Stopping,
    body: Option<Body<'a, '_>>,
) -> Decision {
    let mut decision = Decision {
        values: [0.0; MAX_CHANNELS],
        mask: [0; MAX_CHANNELS],
        channels: channels.len().min(MAX_CHANNELS),
        mode: posted.map_or(skynet_ring::mode::POSITION, |p| p.mode),
        cut_power: false,
        stopping,
        response: None,
        tripped: [None; 8],
        escalated: None,
        advisory: false,
        clamped: [false; MAX_CHANNELS],
    };

    // A stop already under way is not reconsidered. §7.1's ladder is a decision about a fault that
    // has happened; letting the next cycle's clean verdict cancel a ramp would mean a robot that
    // stops only while the condition is still true, which is not a stop.
    if stopping != Stopping::No {
        return continue_ramp(decision, observed, channels, limits, previous);
    }

    // §25: the kernel never blocks on the producer. With nothing posted there is nothing to
    // evaluate and nothing to send, and the mask of zeros is how that is said on the wire. This is
    // not a fault — it is a robot whose policy has not spoken yet, or has stopped speaking, and the
    // watchdog guard is what decides which once there is a command to look at.
    let Some(posted) = posted else {
        return decision;
    };

    let cycle = CommandCycle {
        now_ms,
        command_ms: posted.command_ms,
        sequence: posted.sequence,
        desired: posted.desired,
        previous,
        capability: posted.capability,
        // Nothing on this edition measures separation. §7.2's rule is that the guard reports
        // unavailable rather than passing, and `None` is what produces that.
        nearest_person_m: None,
    };

    let verdict = evaluate_cycle_with_body(&cycle, limits, envelope, last_sequence, body);

    decision.response = verdict.response;
    decision.advisory = verdict.advisory;
    for (slot, guard) in decision.tripped.iter_mut().zip(verdict.tripped.iter()) {
        *slot = Some(*guard);
    }

    // The verdict's command is keyed by channel name and the wire is positional, so the plan's
    // order is what puts a value on the right joint. A channel the verdict does not name is one the
    // kernel is not sending — mask stays zero, which is the hold.
    for (index, channel) in channels.iter().take(MAX_CHANNELS).enumerate() {
        if let Some((_, value)) = verdict.command.iter().find(|(name, _)| name == channel) {
            decision.values[index] = *value as f32;
            decision.mask[index] = 1;
        }
        decision.clamped[index] = verdict
            .clamped
            .iter()
            .any(|entry| entry.channel == *channel);
    }

    // **An advisory verdict is not enforced, and that includes here.**
    //
    // `Tier::enforcement` derives advisory from a tier that cannot promise a clamp, and
    // `evaluate_cycle` already encodes what that means: under `advisory` the verdict's command is
    // the *desired* command rather than the clamped one, precisely so a guest-tier deployment does
    // not look enforced in its own logs. The loop above has already written it.
    //
    // Everything below this line is enforcement — withholding a mask, cutting power, starting a
    // ramp — and running it on an advisory verdict would enforce at guest tier while the record
    // beside it said `advisory: true`. That is the failure §7.4 and §42 both exist to prevent, and
    // it was in this file for an afternoon: the `Hold` arm zeroed the mask on every verdict, so a
    // guest-tier robot withheld a command the kernel had said to forward.
    //
    // The trips and the response are still recorded. Observing and reporting is the whole of what
    // the guest tier does, and it does it here too.
    if verdict.advisory {
        return decision;
    }

    match verdict.response {
        // Clamp forwards what the verdict already reduced; the loop above has written it.
        None | Some(FaultResponse::Clamp) => {}

        // Hold: say nothing, on every channel. The values written above stay visible in the record
        // and none of them is applied.
        Some(FaultResponse::Hold) => {
            decision.mask = [0; MAX_CHANNELS];
        }

        // Cat 0 is the one response that needs no rate: power off, this cycle, nothing sent.
        Some(FaultResponse::StopCat0) => {
            decision.mask = [0; MAX_CHANNELS];
            decision.cut_power = true;
        }

        Some(demanded @ (FaultResponse::StopCat1 | FaultResponse::StopCat2)) => {
            let rampable = channels
                .iter()
                .take(MAX_CHANNELS)
                .any(|channel| ramp_step(limits, channel).is_some());

            if rampable {
                decision.stopping = if demanded == FaultResponse::StopCat1 {
                    Stopping::RampingToOff
                } else {
                    Stopping::RampingToHold
                };
                decision = continue_ramp(decision, observed, channels, limits, previous);
            } else {
                // No jerk ceiling and no per-channel ceiling means no declared rate, so there is
                // nothing to ramp at. See the module header: escalating is the wrong trade and
                // doing it silently is worse.
                decision.mask = [0; MAX_CHANNELS];
                decision.cut_power = true;
                decision.escalated = Some(Escalation {
                    demanded,
                    performed: FaultResponse::StopCat0,
                    because:
                        "no ramp rate is declared: the resolved limits carry no jerk ceiling, \
                              or no channel has one, so a controlled ramp has no rate to run at",
                });
            }
        }
    }

    decision
}

/// One cycle of a ramp already under way, and the arrival at the end of it.
fn continue_ramp(
    mut decision: Decision,
    observed: &Observed<'_>,
    channels: &[&str],
    limits: &ResolvedLimits<'_>,
    previous: Option<&[(&str, f64)]>,
) -> Decision {
    let mut arrived = true;

    for (index, channel) in channels.iter().take(MAX_CHANNELS).enumerate() {
        // What this channel was last commanded. A channel nobody has commanded is already at rest
        // as far as this kernel is concerned, and ramping a value it never sent would be the kernel
        // inventing a setpoint.
        let last = previous
            .and_then(|values| values.iter().find(|(name, _)| name == channel))
            .map(|(_, value)| *value);

        let Some(last) = last else {
            continue;
        };

        match ramp_step(limits, channel) {
            Some(step) => {
                let next = toward_zero(last, step);
                decision.values[index] = next as f32;
                decision.mask[index] = 1;
                if next != 0.0 {
                    arrived = false;
                }
            }
            // A channel with no rate in a ramp that has one elsewhere: it cannot be ramped, so it
            // is held rather than dropped to zero in one step, which would be the jerk the ramp
            // exists to avoid.
            None => {
                decision.mask[index] = 0;
            }
        }
    }

    // §7.1 Cat 2 retains power and engages brakes; Cat 1 removes it. Either way the ramp is over
    // when every rampable channel is at zero, and the state stops being carried.
    if arrived {
        match decision.stopping {
            Stopping::RampingToOff => {
                decision.cut_power = true;
                decision.mask = [0; MAX_CHANNELS];
            }
            Stopping::RampingToHold => {
                decision.mask = [0; MAX_CHANNELS];
            }
            Stopping::No => {}
        }
        decision.stopping = Stopping::No;
    }

    // The observed fault code travels with the decision unchanged; nothing here interprets it.
    let _ = observed;
    decision
}

/// Build the black box record for a decided cycle.
///
/// Separated from [`decide`] because a record is not a decision: the loop knows the tick, what
/// `write_command` returned and which policy digest is running, and none of those is an input to
/// what may be sent.
#[allow(clippy::too_many_arguments)]
pub fn record(
    tick: u64,
    observation: [u8; 32],
    policy: u16,
    capability: u16,
    channels: &[&str],
    observed: &Observed<'_>,
    posted: Option<&Posted<'_, '_>>,
    decision: &Decision,
    rhal_result: i32,
    read_result: i32,
) -> Cycle {
    let width = decision.channels;

    // Every per-channel array in a record is the **declared width**, because a reader indexes them
    // by channel. `encode` refuses anything else, and the first version of this function found that
    // out the hard way: an idle cycle wrote an empty `desired` and the format answered
    // `Shape { field: "desired", expected: 3, found: 0 }`.
    //
    // Padding with zero would have satisfied the check and been the wrong fix. A recorded `desired`
    // of 0.0 says the policy asked for the zero position; on an idle cycle nothing asked for
    // anything. §74.3's rule that zero and unmeasured are different facts applies to a channel
    // exactly as it applies to a counter, and `nearest_person_m` in this same record is `NaN` for
    // the same reason.
    fn to_width(values: &[f32], width: usize) -> Vec<f32> {
        let mut out = vec![f32::NAN; width];
        for (slot, value) in out.iter_mut().zip(values.iter()) {
            *slot = *value;
        }
        out
    }

    // What the policy asked for, before anything reduced it — placed by channel, because the
    // posted list carries only the channels the policy actually commanded (§24's mask).
    let mut desired = vec![f32::NAN; width];
    if let Some(posted) = posted {
        for (index, channel) in channels.iter().take(width).enumerate() {
            if let Some((_, value)) = posted.desired.iter().find(|(name, _)| name == channel) {
                desired[index] = *value as f32;
            }
        }
    }

    Cycle {
        tick,
        observation,
        policy,
        state_flags: observed.flags,
        fault_code: observed.fault_code,
        rhal_result,
        // §26 records what the *read* returned beside what the write did, and until 2026-09-07 it
        // recorded only the write. On a failed read every other field here is a fill value or the
        // previous cycle's data — `observed` is built with `map_or` defaults and the joint arrays
        // are whatever the driver's buffers last held — so a record without this was a record of a
        // healthy robot on the cycle its driver stopped answering.
        read: if read_result == 0 {
            skynet_blackbox::record::ReadOutcome::Ok
        } else {
            skynet_blackbox::record::ReadOutcome::Failed(read_result)
        },
        state_seq: observed.seq,
        state_stamp_ns: observed.stamp_ns,
        // §24 stamps a command when the policy produced it. The ring carries nanoseconds and the
        // kernel works in milliseconds, so this is the one place the two meet.
        command_stamp_ns: posted.map_or(0, |posted| (posted.command_ms * 1_000_000.0) as u64),
        command_seq: posted.map_or(0, |posted| posted.sequence as u32),
        mode: decision.mode as u8,
        advisory: decision.advisory,
        response: decision.response,
        tripped: decision.tripped.iter().flatten().copied().collect(),
        capability,
        // Nothing on this edition measures separation, and NaN is how the format says unmeasured
        // rather than zero — a recorded 0.0 would read as a person touching the robot.
        nearest_person_m: f32::NAN,
        base_pose: observed.base_pose,
        base_twist: observed.base_twist,
        desired,
        command: decision.values[..width]
            .iter()
            .map(|value| *value as f64)
            .collect(),
        mask: decision.mask[..width].to_vec(),
        clamped: decision.clamped[..width].to_vec(),
        // The joint arrays are the driver's and the channel arrays are the plan's, and the format
        // sizes them separately. They are the same width on every body this binary drives today;
        // `to_width` is what keeps a disagreement from being a refused record rather than a short
        // one, and the padding is `NaN` for the reason above.
        position: to_width(observed.position, width),
        velocity: to_width(observed.velocity, width),
        effort: to_width(observed.effort, width),
    }
}
