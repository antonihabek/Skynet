//! Tests for the loop and the cycle.
//!
//! The cases worth having are the ones a robot would otherwise be the first to find: what goes on
//! the wire when nothing is posted, what a hold actually writes, what a stalled producer does, and
//! what a ramp does on a machine that declares no rate to ramp at. None of them needs a driver, a
//! ring in shared memory, or a millisecond of real time.
//!
//! Which guard demands which response is [`skynet_kernel::Guard::response`]'s table and not this
//! crate's to choose, so the tests reach the responses through the guards that really produce them:
//! the **mode gate** demands a hold, and the **watchdog** demands a Cat 2 ramp. Writing a test that
//! asserted a hold from a stalled sequence would be asserting a table that does not exist.

use skynet_kernel::fixed::FixedVec;
use skynet_kernel::{
    Enforcement, Envelope, FaultResponse, Guard, ResolvedLimits, Tier, MAX_CHANNELS,
};
use skynet_ring::{mode, words_for, Command, Ring};
use std::sync::atomic::AtomicU32;

use crate::cycle::{decide, record, Observed, Posted, Stopping};
use crate::driver::Fake;
use crate::run::Host;
use skynet_obs::TimeBase;

const CHANNELS: [&str; 2] = ["shoulder", "elbow"];

/// A fake keeping a simulator's time: `period_ns` per read, from zero (§35.3) — `drivers/mujoco`'s
/// clock, in arrays. Set on the field rather than through a builder on `Fake`, because a `pub fn`
/// only a test calls is the shape `check-production-callers` exists to refuse.
fn simulated_fake(joints: usize, period_ns: u64) -> Fake {
    let mut body = Fake::new(joints);
    body.simulated_period_ns = Some(period_ns);
    body
}
const CEILINGS: [(&str, f64); 2] = [("shoulder", 1.0), ("elbow", 1.0)];

// §86.3's map, which almost every case here wants empty. A `thread_local` rather than a parameter
// on `limits`, because two tests care and adding an argument to a helper twenty cases call would
// put an `&[]` in twenty places to say nothing.
thread_local! {
    static JOINT_MODES_CELL: core::cell::Cell<&'static [(&'static str, &'static str)]> =
        const { core::cell::Cell::new(&[]) };
}

struct JointModes;

impl JointModes {
    fn get(&self) -> &'static [(&'static str, &'static str)] {
        JOINT_MODES_CELL.with(core::cell::Cell::get)
    }

    /// Set for the rest of this test's thread. Tests run one per thread, so this does not leak.
    fn set(&self, modes: &'static [(&'static str, &'static str)]) {
        JOINT_MODES_CELL.with(|cell| cell.set(modes));
    }
}

const JOINT_MODES: JointModes = JointModes;

/// Limits over the two channels, with the guards and jerk fraction a case needs.
fn limits<'a>(
    per_channel: &'a [(&'a str, f64)],
    guards: &[Guard],
    jerk: Option<f64>,
    grants: &'a [&'a str],
) -> ResolvedLimits<'a> {
    let mut active: FixedVec<Guard, 8> = FixedVec::new();
    for guard in guards {
        active.push(*guard);
    }
    ResolvedLimits {
        tier: Tier::Host,
        enforcement: Enforcement::Enforcing,
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        min_control_hz: Some(100.0),
        max_delta_per_cycle: jerk,
        proximity: None,
        per_channel,
        joint_modes: JOINT_MODES.get(),
        active_guards: active,
        unavailable_guards: FixedVec::new(),
        inert_guards: FixedVec::new(),
        source: Some("test"),
        basis: None,
        notices: FixedVec::new(),
        grants,
        frames: None,
        workspace: &[],
        self_collision_margin_m: None,
    }
}

/// The same limits at the tier that cannot enforce.
///
/// `Tier::Guest` derives `Enforcement::Advisory`, and nothing may pass an enforcement in: the
/// kernel derives it so that a tier which cannot promise a clamp cannot be configured to claim one.
fn advisory_limits<'a>(
    per_channel: &'a [(&'a str, f64)],
    guards: &[Guard],
    jerk: Option<f64>,
    grants: &'a [&'a str],
) -> ResolvedLimits<'a> {
    let mut limits = limits(per_channel, guards, jerk, grants);
    limits.tier = Tier::Guest;
    limits.enforcement = Tier::Guest.enforcement();
    limits
}

fn observed(zeros: &[f32]) -> Observed<'_> {
    Observed {
        stamp_ns: 1_000_000,
        seq: 1,
        flags: 0,
        fault_code: 0,
        position: zeros,
        velocity: zeros,
        effort: zeros,
        base_pose: [f32::NAN; 7],
        base_twist: [0.0; 6],
    }
}

// ── The translation a robot would otherwise be first to find ────────────────

/// §24: a hold is a mask of zeros. The values are irrelevant and the mask is everything.
///
/// If this ever fails by writing a mask of ones with zero values, a position-controlled arm slams
/// every joint to its origin and a torque-controlled one goes limp. It is the single most
/// consequential line in the crate.
///
/// The hold is reached through the mode gate: a command declaring a capability the operator never
/// granted. §7.1's table is what says that demands a hold.
#[test]
fn a_hold_writes_no_channel() {
    let zeros = [0.0f32; 2];
    let limits = limits(&CEILINGS, &[Guard::ModeGate], Some(0.1), &[]);

    let desired = [("elbow", 0.5), ("shoulder", 0.5)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 8,
        mode: mode::POSITION,
        capability: Some("actuation.arm"),
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        None,
        Some(7),
        Stopping::No,
        None,
    );

    assert_eq!(decision.response, Some(FaultResponse::Hold));
    assert!(
        decision.mask[..2].iter().all(|entry| *entry == 0),
        "a hold must say nothing on every channel, and this said something",
    );
}

/// Nothing posted is not a fault, and it is not a command of zeros either.
#[test]
fn an_empty_ring_writes_no_channel_and_trips_nothing() {
    let zeros = [0.0f32; 2];
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);

    let decision = decide(
        1.0,
        &observed(&zeros),
        None,
        &CHANNELS,
        &limits,
        None,
        None,
        None,
        Stopping::No,
        None,
    );

    assert_eq!(decision.response, None, "no command is not a guard trip");
    assert!(decision.mask[..2].iter().all(|entry| *entry == 0));
    assert!(!decision.cut_power);
}

/// A command outside its channel's ceiling is reduced to the ceiling and forwarded.
#[test]
fn a_command_over_its_ceiling_is_clamped_and_still_sent() {
    let zeros = [0.0f32; 2];
    let ceilings = [("shoulder", 0.5), ("elbow", 10.0)];
    let limits = limits(&ceilings, &[Guard::Envelope], Some(1.0), &[]);

    let desired = [("elbow", 0.1), ("shoulder", 5.0)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 3,
        mode: mode::VELOCITY,
        capability: None,
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        None,
        Some(2),
        Stopping::No,
        None,
    );

    assert_eq!(decision.response, Some(FaultResponse::Clamp));
    assert_eq!(decision.mask[0], 1, "a clamp is still a command");
    assert!(
        (decision.values[0] - 0.5).abs() < 1e-6,
        "5.0 was reduced to the ceiling of 0.5, not dropped",
    );
    assert!(
        decision.clamped[0],
        "and the shoulder is recorded as clamped"
    );
    assert!(!decision.clamped[1], "while the elbow was inside its own");
}

// ── Advisory, which is every robot today ────────────────────────────────────

/// An advisory verdict is forwarded, not withheld.
///
/// **Regression.** This module's `decide` zeroed the mask on every `Hold`, including advisory ones,
/// so a guest-tier robot withheld a command the kernel had said to forward while the record beside
/// it said `advisory: true`. §7.4 and §42 both exist to stop exactly that, and what found it was a
/// conformance test in `skynetd-io` whose expectations are written against the kernel.
#[test]
fn an_advisory_hold_forwards_the_command_it_would_have_withheld() {
    let zeros = [0.0f32; 2];
    let limits = advisory_limits(&CEILINGS, &[Guard::ModeGate], Some(0.1), &[]);

    let desired = [("elbow", 0.5), ("shoulder", 0.5)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 8,
        mode: mode::POSITION,
        capability: Some("actuation.arm"),
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        None,
        Some(7),
        Stopping::No,
        None,
    );

    assert!(decision.advisory, "guest tier cannot enforce");
    assert_eq!(
        decision.response,
        Some(FaultResponse::Hold),
        "the guard still trips and the trip is still recorded",
    );
    assert!(
        decision.mask[..2].iter().any(|entry| *entry != 0),
        "an advisory hold must forward the command: withholding it enforces at a tier that has \
         never claimed to enforce",
    );
    assert!(!decision.cut_power);
}

/// An advisory stop starts no ramp and cuts no power.
#[test]
fn an_advisory_stop_enforces_nothing() {
    let zeros = [0.0f32; 2];
    let limits = advisory_limits(&CEILINGS, &[Guard::Watchdog], Some(0.25), &[]);
    let previous = [("shoulder", 1.0), ("elbow", -0.5)];

    let desired = [("elbow", 0.5), ("shoulder", 0.5)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 7,
        mode: mode::VELOCITY,
        capability: None,
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        Some(&previous),
        Some(7),
        Stopping::No,
        None,
    );

    assert_eq!(decision.response, Some(FaultResponse::StopCat2));
    assert_eq!(
        decision.stopping,
        Stopping::No,
        "an advisory verdict starts no ramp",
    );
    assert!(!decision.cut_power, "and removes no power");
    assert!(decision.escalated.is_none());
}

// ── The ramp, and the machine that cannot perform one ───────────────────────

/// A stalled producer is a Cat 2 stop, and a Cat 2 stop with a declared rate is a ramp.
#[test]
fn a_stalled_producer_ramps_rather_than_dropping_to_zero() {
    let zeros = [0.0f32; 2];
    // Ceiling 1.0 with a jerk fraction of 0.25 gives a step of 0.25 per cycle.
    let limits = limits(&CEILINGS, &[Guard::Watchdog], Some(0.25), &[]);
    let previous = [("shoulder", 1.0), ("elbow", -0.5)];

    let desired = [("elbow", 0.5), ("shoulder", 0.5)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 7,
        mode: mode::VELOCITY,
        capability: None,
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        Some(&previous),
        // The same sequence as last cycle: §24's stalled producer.
        Some(7),
        Stopping::No,
        None,
    );

    assert_eq!(decision.response, Some(FaultResponse::StopCat2));
    assert_eq!(
        decision.stopping,
        Stopping::RampingToHold,
        "a Cat 2 with a rate becomes a ramp that outlives this cycle",
    );
    assert!((decision.values[0] - 0.75).abs() < 1e-6, "1.0 - 0.25");
    assert!((decision.values[1] - (-0.25)).abs() < 1e-6, "-0.5 + 0.25");
    assert!(!decision.cut_power, "Cat 2 retains power");
}

/// A ramp in progress is not reconsidered by a later clean verdict.
#[test]
fn a_ramp_runs_to_the_end_even_when_the_fault_clears() {
    let zeros = [0.0f32; 2];
    let limits = limits(&CEILINGS, &[Guard::Watchdog], Some(0.25), &[]);
    let previous = [("shoulder", 1.0), ("elbow", 0.0)];

    // A perfectly good command, arriving mid-ramp.
    let desired = [("elbow", 0.1), ("shoulder", 0.1)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 99,
        mode: mode::VELOCITY,
        capability: None,
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        Some(&previous),
        Some(98),
        Stopping::RampingToHold,
        None,
    );

    assert!(
        (decision.values[0] - 0.75).abs() < 1e-6,
        "the ramp continued; the new command did not cancel it",
    );
    assert_eq!(decision.stopping, Stopping::RampingToHold);
}

/// The last step of a Cat 1 ramp is where power is removed — not the first.
#[test]
fn a_cat_one_ramp_cuts_power_only_on_arrival() {
    let zeros = [0.0f32; 2];
    let limits = limits(&CEILINGS, &[Guard::Watchdog], Some(0.25), &[]);

    // Both channels are within one step of zero, so this cycle is the one that arrives.
    let previous = [("shoulder", 0.1), ("elbow", -0.2)];
    let decision = decide(
        1.0,
        &observed(&zeros),
        None,
        &CHANNELS,
        &limits,
        None,
        Some(&previous),
        None,
        Stopping::RampingToOff,
        None,
    );

    assert!(decision.cut_power, "arrival is where Cat 1 removes power");
    assert_eq!(decision.stopping, Stopping::No, "the ramp is over");
    assert!(decision.mask[..2].iter().all(|entry| *entry == 0));
}

/// A body that declares no jerk ceiling has no rate to ramp at, and the escalation is recorded.
///
/// This is the case the module header argues about at length: escalating a Cat 2 to a Cat 0 is the
/// wrong trade for a humanoid holding a crate, and doing it *silently* is worse. The assertion that
/// matters is the second one — that the record says what was performed and why — because without it
/// an operator reading a black box concludes the guard demanded a Cat 0.
#[test]
fn a_ramp_with_no_declared_rate_escalates_and_says_so() {
    let zeros = [0.0f32; 2];
    let limits = limits(&CEILINGS, &[Guard::Watchdog], None, &[]);
    let previous = [("shoulder", 1.0), ("elbow", -0.5)];

    let desired = [("elbow", 0.5), ("shoulder", 0.5)];
    let posted = Posted {
        desired: &desired,
        command_ms: 1.0,
        sequence: 7,
        mode: mode::VELOCITY,
        capability: None,
    };

    let decision = decide(
        1.0,
        &observed(&zeros),
        Some(&posted),
        &CHANNELS,
        &limits,
        None,
        Some(&previous),
        Some(7),
        Stopping::No,
        None,
    );

    assert_eq!(
        decision.response,
        Some(FaultResponse::StopCat2),
        "the guard demanded what it always demands",
    );
    assert!(decision.cut_power, "and a Cat 0 is what could be performed");

    let escalation = decision.escalated.expect("the escalation must be recorded");
    assert_eq!(escalation.demanded, FaultResponse::StopCat2);
    assert_eq!(escalation.performed, FaultResponse::StopCat0);
    assert!(
        escalation.because.contains("no ramp rate"),
        "the reason has to name the missing input, not just report a difference",
    );
    assert!(decision.mask[..2].iter().all(|entry| *entry == 0));
}

// ── The loop ────────────────────────────────────────────────────────────────

/// A ring is a slice of words; the suite backs it with an array, as `skynet-ring`'s own tests do.
///
/// Sized with the crate's own `words_for` rather than a round number, because a slot is 4 KiB and
/// eight of them is not the 4,096 words that looks like the right order of magnitude.
const SLOTS: usize = 8;

fn ring_words() -> Vec<AtomicU32> {
    (0..words_for(SLOTS)).map(|_| AtomicU32::new(0)).collect()
}

/// Every cycle produces a record, including the ones where nothing happened.
///
/// §15's `field_conformance` is a ratio over cycles, and a loop that recorded only the interesting
/// ones would compute it over a denominator that flattered the robot.
#[test]
fn every_cycle_is_recorded_even_with_an_empty_ring() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    let mut ticks = Vec::new();
    for cycle in 0..5u64 {
        let turn = host.step(
            cycle * 10_000_000,
            &mut body,
            &mut consumer,
            &limits,
            None::<&Envelope<'_>>,
        );
        ticks.push(turn.cycle.tick);
    }

    assert_eq!(ticks, vec![1, 2, 3, 4, 5], "one record per cycle, in order");
    assert_eq!(host.summary().cycles, 5);
    assert_eq!(host.summary().idle, 5, "the policy never spoke");
    assert_eq!(host.summary().interventions, 0, "and nothing tripped");
    assert_eq!(
        body.written.len(),
        5,
        "a cycle with nothing posted still writes a hold, or the driver's own command-loss timeout \
         decides the response instead of the kernel",
    );
    assert!(
        body.written
            .iter()
            .all(|written| written.mask.iter().all(|entry| *entry == 0)),
        "and every one of those writes is a mask of zeros",
    );
}

/// A command posted on the ring reaches the driver.
#[test]
fn a_posted_command_crosses_the_ring_and_reaches_the_body() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let mut command = Command {
        stamp_ns: 1_000_000,
        seq: 1,
        mode: mode::POSITION,
        channels: 2,
        ..Command::default()
    };
    command.values[0] = 0.4;
    command.values[1] = -0.3;
    command.mask[0] = 1;
    command.mask[1] = 1;
    producer.publish(&command).expect("the ring accepts it");

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert_eq!(turn.decision.mask[0], 1, "the channel was commanded");
    assert!((turn.decision.values[0] - 0.4).abs() < 1e-6);
    assert!((turn.decision.values[1] - (-0.3)).abs() < 1e-6);
    assert_eq!(turn.cycle.command_seq, 1);
    assert_eq!(
        turn.cycle.desired.len(),
        2,
        "one entry per channel: the format indexes these by channel and refuses any other width",
    );
    assert!(
        (turn.cycle.desired[0] - 0.4).abs() < 1e-6,
        "the record carries what the policy asked for, not only what was sent",
    );
    assert!((body.position[0] - 0.4).abs() < 1e-6, "and the body moved");
}

/// §86.1: a body whose joints disagree about mode is commanded with a per-joint array.
///
/// The map is `ResolvedLimits`', which is the transport plan's, which is the body's — so the array
/// the driver receives is derived from the descriptor rather than from anything this loop decided.
/// The control is the case below it: the same body with both joints in one mode sends no array at
/// all, because a v1 driver must not be handed a flag it does not read.
#[test]
fn a_mixed_mode_body_is_commanded_one_mode_per_joint() {
    const MIXED: [(&str, &str); 2] = [("shoulder", "position"), ("elbow", "velocity")];
    JOINT_MODES.set(&MIXED);

    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let mut command = Command {
        stamp_ns: 1_000_000,
        seq: 1,
        mode: mode::POSITION,
        channels: 2,
        ..Command::default()
    };
    command.values[0] = 0.4;
    command.values[1] = -0.3;
    command.mask[0] = 1;
    command.mask[1] = 1;
    producer.publish(&command).expect("the ring accepts it");

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    let written = body.last().expect("the body was commanded");
    assert_eq!(
        written.modes,
        &[skynet_rhal::jmode::POSITION, skynet_rhal::jmode::VELOCITY],
        "each joint carries the mode the plan bound it in, not the command's own",
    );

    JOINT_MODES.set(&[]);
}

/// F170: with nothing posted, the hold goes out in the mode the plan binds every joint in. Held to
/// a control — with no plan it is position, as it always was.
#[test]
fn a_hold_with_nothing_posted_is_in_the_plans_mode() {
    const TORQUE: [(&str, &str); 2] = [("shoulder", "torque"), ("elbow", "torque")];
    let written_mode = |modes: &'static [(&'static str, &'static str)]| {
        JOINT_MODES.set(modes);
        let words = ring_words();
        let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
        let mut consumer = ring.consumer();
        let mut body = Fake::new(2);
        let limits = limits(&CEILINGS, &[Guard::Envelope], None, &[]);
        let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
        host.step(
            1_000_000,
            &mut body,
            &mut consumer,
            &limits,
            None::<&Envelope<'_>>,
        );
        let written = body.last().expect("the body was held").clone();
        JOINT_MODES.set(&[]);
        assert!(
            written.mask.iter().all(|&held| held == 0),
            "a hold, every joint"
        );
        written.mode
    };
    assert_eq!(written_mode(&TORQUE), mode::TORQUE as u8);
    assert_eq!(written_mode(&[]), mode::POSITION as u8);
}

/// The control for the case above: joints that agree send no per-joint array.
#[test]
fn a_single_mode_body_sends_no_per_joint_array() {
    const AGREED: [(&str, &str); 2] = [("shoulder", "position"), ("elbow", "position")];
    JOINT_MODES.set(&AGREED);

    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let mut command = Command {
        stamp_ns: 1_000_000,
        seq: 1,
        mode: mode::POSITION,
        channels: 2,
        ..Command::default()
    };
    command.mask[0] = 1;
    command.mask[1] = 1;
    producer.publish(&command).expect("the ring accepts it");

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    let written = body.last().expect("the body was commanded");
    assert!(
        written.modes.is_empty(),
        "one mode for every joint is a body-wide command, and sending an array would hand a driver \
         written before §86 a flag it does not read for no gain",
    );

    JOINT_MODES.set(&[]);
}

/// A channel the policy masks out is one it is not commanding, and never appears as a desired zero.
#[test]
fn a_masked_channel_is_absent_from_desired_rather_than_zero() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let mut command = Command {
        stamp_ns: 1_000_000,
        seq: 1,
        mode: mode::POSITION,
        channels: 2,
        ..Command::default()
    };
    command.values[0] = 0.4;
    command.mask[0] = 1;
    // Channel 1 is masked out: the policy is saying nothing about the elbow.
    command.mask[1] = 0;
    producer.publish(&command).expect("the ring accepts it");

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    // Every per-channel array is the declared width — the format refuses anything else — so the
    // channel the policy said nothing about is `NaN` rather than absent or zero. A recorded 0.0
    // would say the policy asked for the zero position, which is the opposite of saying nothing.
    assert_eq!(turn.cycle.desired.len(), CHANNELS.len());
    assert!(
        (turn.cycle.desired[0] - 0.4).abs() < 1e-6,
        "the shoulder was commanded",
    );
    assert!(
        turn.cycle.desired[1].is_nan(),
        "and the elbow was not asked for at all, which is not the same as asked for zero",
    );
    assert_eq!(turn.decision.mask[1], 0, "and the elbow is still held");
    assert!(
        (body.position[1]).abs() < 1e-6,
        "a masked channel is held, not driven to the value beside it",
    );
}

/// A failed state read does not skip the cycle: the record still exists and says so.
#[test]
fn a_failed_read_still_produces_a_record() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();

    let mut body = Fake::new(2);
    body.read_fails = true;
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    let turn = host.step(
        1_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert!(turn.read_error.is_some());
    assert_eq!(turn.cycle.tick, 1, "the cycle happened and is numbered");
    assert_eq!(host.summary().read_failures, 1);
    assert_eq!(host.summary().cycles, 1);

    // §24, and the reason the record needed a field. Until 2026-09-07 this cycle was encoded with
    // zeroed flags, a zero fault code, a zero sequence and the previous cycle's joint positions —
    // every one of them a legal value on a healthy robot — so a recording of a driver that had
    // stopped answering was indistinguishable from a recording of one that had not. The counters
    // above knew, and they live in this process's memory and die with it; the black box outlives
    // the robot and is what an investigator has.
    assert_eq!(
        turn.cycle.read,
        skynet_blackbox::record::ReadOutcome::Failed(-110),
        "the record has to carry what the read returned, not only what the write did"
    );
    assert!(!turn.cycle.read.measured());
    assert_eq!(
        turn.read_error.as_ref().map(|failure| failure.code),
        Some(-110),
        "and the code an operator is shown is the code the recording keeps"
    );
}

/// A body with no emergency stop cannot perform Cat 0, and the loop reports it rather than counting
/// a stop it did not achieve.
#[test]
fn a_body_with_no_estop_reports_it_rather_than_counting_a_stop() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    // A command that will trip the watchdog on the second cycle by not advancing.
    let mut command = Command {
        stamp_ns: 1_000_000,
        seq: 4,
        mode: mode::VELOCITY,
        channels: 2,
        ..Command::default()
    };
    command.values[0] = 0.5;
    command.mask[0] = 1;
    producer.publish(&command).expect("the ring accepts it");

    let mut body = Fake::new(2);
    body.stop_available = false;

    // No jerk ceiling, so the Cat 2 the watchdog demands escalates to a Cat 0 this body cannot do.
    let limits = limits(&CEILINGS, &[Guard::Watchdog], None, &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    // First cycle establishes the sequence; the second sees it unchanged.
    host.step(
        1_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert!(turn.decision.cut_power, "a Cat 0 was decided");
    assert!(
        turn.stop_unavailable,
        "and this body cannot perform one, which is a fact for a person",
    );
    assert_eq!(
        host.summary().stops,
        0,
        "a stop that could not be performed is not counted as one",
    );
    assert_eq!(body.stops, 0);
}

/// A driver that refuses the command is counted, and the code it returned is in the record.
#[test]
fn a_refused_command_is_recorded_with_the_drivers_own_code() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();

    let mut body = Fake::new(2);
    body.write_result = -13;
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    let turn = host.step(
        1_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert_eq!(turn.cycle.rhal_result, -13, "surfaced verbatim, per §24");
    assert_eq!(host.summary().write_failures, 1);
    let (cycle, code, _mode) = host
        .summary()
        .first_write_failure
        .expect("the first is kept");
    assert_eq!(
        (cycle, code),
        (1, -13),
        "which write, and what the driver said"
    );
}

// ── The record ──────────────────────────────────────────────────────────────

/// Unmeasured is `NaN`, never zero.
///
/// A recorded `0.0` for `nearest_person_m` reads as a person touching the robot, which is the
/// opposite of "nothing is measuring separation".
#[test]
fn an_unmeasured_separation_is_nan_and_not_zero() {
    let zeros = [0.0f32; 2];
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let decision = decide(
        1.0,
        &observed(&zeros),
        None,
        &CHANNELS,
        &limits,
        None,
        None,
        None,
        Stopping::No,
        None,
    );

    let cycle = record(
        1,
        [0u8; 32],
        0,
        skynet_blackbox::record::NO_CAPABILITY,
        &CHANNELS,
        &observed(&zeros),
        None,
        &decision,
        0,
        0,
    );

    assert!(cycle.nearest_person_m.is_nan());
    assert_eq!(cycle.desired.len(), CHANNELS.len());
    assert!(
        cycle.desired.iter().all(|value| value.is_nan()),
        "nothing was posted, so every channel is unmeasured — not a row of zeros",
    );
    assert_eq!(cycle.mask.len(), CHANNELS.len());
    assert_eq!(cycle.clamped.len(), CHANNELS.len());
    assert!(!cycle.intervened(), "no guard tripped");
    assert!(MAX_CHANNELS >= CHANNELS.len());
}

// ── The black box ───────────────────────────────────────────────────────────

/// A recorder over a scratch directory, with the shape of the two-channel fixture body.
fn recorder(name: &str) -> (std::path::PathBuf, crate::recorder::Recorder) {
    let dir = std::env::temp_dir().join(format!("skynet-recorder-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let shape = skynet_blackbox::record::Shape {
        joints: CHANNELS.len() as u16,
        channels: CHANNELS.len() as u16,
    };
    let meta = skynet_blackbox::ring::Meta {
        robot_id: String::new(),
        embodiment: [0u8; 32],
        os_image: [0u8; 32],
        tier: Tier::Host as u8,
        enforcement: Enforcement::Enforcing as u8,
        simulator: false,
        wall_epoch_ns: 0,
        mono_ns: 0,
        channels: CHANNELS.iter().map(|name| name.to_string()).collect(),
    };
    // Small, so the test writes a file measured in kilobytes rather than in tens of megabytes.
    let recorder = crate::recorder::Recorder::open(&dir, meta, shape, 64 * 1024)
        .expect("a fresh directory has no writer");
    (dir, recorder)
}

/// Every cycle the loop turns reaches the ring, and reads back as a cycle.
///
/// The first recording of control cycles this repository has produced. What it establishes is that
/// the record survives the round trip — §13's ring holding what §25's loop decided — and not that
/// anything moved: the ring the loop reads is empty, so all thirty are holds.
#[test]
fn every_cycle_the_loop_turns_reaches_the_ring() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    let (dir, mut recorder) = recorder("cycles");

    let first = recorder.next_sequence();
    for cycle in 0..30u64 {
        let turn = host.step(
            cycle * 5_000_000,
            &mut body,
            &mut consumer,
            &limits,
            None::<&Envelope<'_>>,
        );
        recorder
            .cycle(cycle * 5_000_000, &turn.cycle)
            .expect("the ring accepts a cycle");
    }

    let store = recorder.read().expect("the ring reads back");
    let opened = skynet_blackbox::ring::Ring::open(&store).expect("the header opens");
    let tally = skynet_blackbox::tally::tally(&opened, &store, first);

    assert_eq!(tally.cycles, 30, "thirty cycles turned, thirty recorded");
    assert_eq!(
        tally.interventions, 0,
        "and none of them tripped a guard, because nothing was commanded",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A sealed window is a file, and it holds what the ring held.
#[test]
fn the_window_seals_into_a_file() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();

    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    let (dir, mut recorder) = recorder("seal");

    for cycle in 0..5u64 {
        let turn = host.step(
            cycle * 5_000_000,
            &mut body,
            &mut consumer,
            &limits,
            None::<&Envelope<'_>>,
        );
        recorder
            .cycle(cycle * 5_000_000, &turn.cycle)
            .expect("recorded");
    }

    let (sealed, path) = recorder
        .seal(skynet_blackbox::seal::Reason::Shutdown, 1_000, 1_000)
        .expect("the window seals");

    assert!(path.exists(), "a sealed window is a file on the disk");
    assert!(
        !sealed.bytes.is_empty(),
        "and the file has the recording in it",
    );
    assert!(
        path.to_string_lossy().ends_with("shutdown.mcap"),
        "named by the reason it was sealed: {}",
        path.display(),
    );

    drop(recorder);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sealed window is never overwritten, and two windows sealed at one kernel instant are two files.
///
/// A simulation's time starts at zero every run (§35.3), so two runs of the same length sealed into
/// one directory seal at the same kernel instant. The file was named by that instant for the first
/// run of F70's live gate, and written with `File::create` — which replaces whatever held the name.
/// So the name is the machine's clock, the kernel's time is inside the window, and a name that is
/// taken anyway is refused rather than overwritten.
#[test]
fn a_sealed_window_is_never_overwritten() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();
    let mut body = simulated_fake(2, 10_000_000);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Simulated);
    let (dir, mut recorder) = recorder("never-overwritten");

    let turn = host.step(
        AN_HOUR_NS,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    recorder.cycle(turn.now_ns, &turn.cycle).expect("recorded");

    let (_, first) = recorder
        .seal(
            skynet_blackbox::seal::Reason::Shutdown,
            turn.now_ns,
            AN_HOUR_NS,
        )
        .expect("the first window seals");
    let kept = std::fs::read(&first).expect("the first window is a file");

    // The same kernel instant, a later machine instant: a second run of the same length.
    let (_, second) = recorder
        .seal(
            skynet_blackbox::seal::Reason::Shutdown,
            turn.now_ns,
            AN_HOUR_NS + 1,
        )
        .expect("a second window at the same simulated instant seals beside the first");
    assert_ne!(first, second, "two windows are two files");

    let refused = recorder.seal(
        skynet_blackbox::seal::Reason::Shutdown,
        turn.now_ns,
        AN_HOUR_NS,
    );
    let message = refused
        .expect_err("a name already holding a window is refused")
        .to_string();
    assert!(message.contains("never overwritten"), "{message}");
    assert_eq!(
        std::fs::read(&first).expect("still there"),
        kept,
        "and the window that held it is byte for byte what was sealed",
    );

    drop(recorder);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A second writer on one ring is refused, and told who holds it.
///
/// The alternative is the failure this whole mechanism exists to prevent. `Ring` keeps its next
/// sequence in memory, so two writers hand out the same sequence, write two records into one slot,
/// and produce a recording that looks intact and is not — discovered by an investigator, months
/// later, in the file that was supposed to answer the question.
#[test]
fn a_second_writer_attaches_where_it_can_and_is_refused_where_it_cannot() {
    let (dir, first) = recorder("lock");

    let shape = skynet_blackbox::record::Shape {
        joints: CHANNELS.len() as u16,
        channels: CHANNELS.len() as u16,
    };
    let meta = skynet_blackbox::ring::Meta {
        robot_id: String::new(),
        embodiment: [0u8; 32],
        os_image: [0u8; 32],
        tier: Tier::Host as u8,
        enforcement: Enforcement::Enforcing as u8,
        simulator: false,
        wall_epoch_ns: 0,
        mono_ns: 0,
        channels: CHANNELS.iter().map(|name| name.to_string()).collect(),
    };

    let second = crate::recorder::Recorder::open(&dir, meta.clone(), shape, 64 * 1024);

    #[cfg(target_os = "linux")]
    {
        let second = second.expect("a second writer attaches to the recording the first created");
        assert_eq!(
            second.role(),
            skynet_lock::Role::Attached,
            "the second writer does not become a second creator",
        );
        assert!(
            first.next_sequence() >= 1 && second.next_sequence() >= 1,
            "both read the same counter",
        );
        // The property the whole mechanism is for: the two never take the same number.
        // A real cycle to write, from the loop rather than hand-built: a record this test
        // invented could be one the encoder would reject on a robot.
        let words = ring_words();
        let command_ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
        let mut consumer = command_ring.consumer();
        let mut body = Fake::new(2);
        let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
        let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
        let turn = host.step(0, &mut body, &mut consumer, &limits, None::<&Envelope<'_>>);

        let mut second = second;
        let mut first = first;
        let mut taken = Vec::new();
        for tick in 0..8u64 {
            taken.push(
                first
                    .cycle(tick * 1_000, &turn.cycle)
                    .expect("the creator writes"),
            );
            taken.push(
                second
                    .cycle(tick * 1_000, &turn.cycle)
                    .expect("the attacher writes"),
            );
        }
        taken.sort_unstable();
        assert_eq!(
            taken,
            (1..=16).collect::<Vec<u64>>(),
            "sixteen records took sixteen sequence numbers, each exactly once",
        );

        // And the recording holds all sixteen, in order, which is the assertion that would fail if
        // two of them had gone into one slot.
        let store = first.read().expect("the ring reads back");
        let ring = skynet_blackbox::ring::Ring::open(&store).expect("a ring");
        assert_eq!(
            ring.ordered(&store).len(),
            16,
            "every claimed slot holds a record of its own",
        );

        assert!(
            second
                .seal(skynet_blackbox::seal::Reason::Shutdown, 0, 0)
                .is_err(),
            "an attacher sealing would be a second reader of a file a writer is in the middle of",
        );
        drop(second);
        drop(first);
    }

    #[cfg(not(target_os = "linux"))]
    {
        match second {
            Err(crate::recorder::RecorderError::Attach(error)) => {
                let said = error.to_string();
                assert!(
                    said.contains("skynetk"),
                    "the refusal has to name who holds the ring: {said}",
                );
                assert!(
                    said.contains("Linux"),
                    "and say that the missing shared counter is why, rather than implying the \
                     other process is at fault: {said}",
                );
            }
            Err(other) => panic!("refused for the wrong reason: {other}"),
            Ok(_) => panic!("two writers with no shared counter were allowed onto one ring"),
        }
        drop(first);
    }

    // Releasing the creator lets the next process create, so this is a lock and not a permanent
    // refusal.
    let third = crate::recorder::Recorder::open(&dir, meta, shape, 64 * 1024);
    assert!(third.is_ok(), "the lock is released when its holder goes");

    drop(third);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A loop cannot attach to a ring shaped for a different body, and the refusal says why.
///
/// The asymmetry with `skynetd` is the point: that recorder writes events and gaps, which carry no
/// per-joint array and fit any record size, so it takes the ring it finds. A cycle carries a value
/// per joint and per channel, so it does not.
#[test]
fn a_loop_will_not_attach_to_a_ring_shaped_for_another_body() {
    let dir = std::env::temp_dir().join("skynet-recorder-shape");
    let _ = std::fs::remove_dir_all(&dir);

    let channels: Vec<String> = CHANNELS.iter().map(|name| name.to_string()).collect();
    let meta = skynet_blackbox::ring::Meta {
        robot_id: String::new(),
        embodiment: [0u8; 32],
        os_image: [0u8; 32],
        tier: Tier::Host as u8,
        enforcement: Enforcement::Enforcing as u8,
        simulator: false,
        wall_epoch_ns: 0,
        mono_ns: 0,
        channels: channels.clone(),
    };

    // A creator that recorded a two-joint arm.
    let first = crate::recorder::Recorder::open(
        &dir,
        meta.clone(),
        skynet_blackbox::record::Shape {
            joints: 2,
            channels: 2,
        },
        64 * 1024,
    )
    .expect("a fresh directory");

    let second = crate::recorder::Recorder::open(
        &dir,
        meta,
        skynet_blackbox::record::Shape {
            joints: 30,
            channels: 4,
        },
        64 * 1024,
    );

    match second {
        Err(crate::recorder::RecorderError::Shape {
            found,
            wanted,
            holder,
        }) => {
            assert_eq!(found.joints, 2);
            assert_eq!(wanted.joints, 30);
            assert!(holder.contains("skynetk"), "{holder}");
        }
        Err(other) => panic!("refused for the wrong reason: {other}"),
        Ok(_) => panic!("a ring holding two joints accepted a thirty-joint cycle"),
    }

    drop(first);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A window where a policy commanded, and a guard tripped, and both are counted off the ring.
///
/// The other black box test records thirty holds; this is the one that matters, because §74's five
/// counters and §15's field conformance have never had a numerator that came from a machine. The
/// policy asks for a velocity outside the channel ceiling on every cycle, the envelope guard clamps
/// it, and `tally` counts the clamps off the records rather than off anything this test kept.
#[test]
fn a_commanded_window_counts_its_own_trips() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let mut body = Fake::new(2);
    // A ceiling of 0.5 against a command of 5.0: the envelope guard clamps, every cycle.
    let ceilings = [("shoulder", 0.5), ("elbow", 0.5)];
    let limits = limits(&ceilings, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    let (dir, mut recorder) = recorder("commanded");

    let first = recorder.next_sequence();
    for cycle in 0..20u64 {
        // A new sequence every cycle, or the watchdog would trip as well and the count would be
        // about two guards rather than the one under test.
        let mut command = Command {
            stamp_ns: cycle * 5_000_000,
            seq: cycle + 1,
            mode: mode::VELOCITY,
            channels: 2,
            ..Command::default()
        };
        command.values[0] = 5.0;
        command.values[1] = 5.0;
        command.mask[0] = 1;
        command.mask[1] = 1;
        producer.publish(&command).expect("the ring accepts it");

        let turn = host.step(
            cycle * 5_000_000,
            &mut body,
            &mut consumer,
            &limits,
            None::<&Envelope<'_>>,
        );
        recorder
            .cycle(cycle * 5_000_000, &turn.cycle)
            .expect("recorded");
    }

    let store = recorder.read().expect("the ring reads back");
    let opened = skynet_blackbox::ring::Ring::open(&store).expect("the header opens");
    let tally = skynet_blackbox::tally::tally(&opened, &store, first);

    assert!(tally.commands, "a ring with channels can hold a cycle");
    assert_eq!(tally.cycles, 20);
    assert_eq!(
        tally.interventions, 20,
        "every cycle asked for ten times the ceiling, so the kernel intervened in every one",
    );
    assert!(
        tally.guard_trips >= 20,
        "and the trips are counted off the records: {}",
        tally.guard_trips,
    );
    assert!(
        tally.clamps >= 20,
        "two channels clamped on each of twenty cycles: {}",
        tally.clamps,
    );

    // §15's figure, with a numerator and a denominator that both came from a machine. Zero here,
    // and that is the correct answer for a policy that spent the whole window outside the envelope.
    let conformance = 1.0 - (tally.interventions as f64 / tally.cycles as f64);
    assert!(
        conformance.abs() < 1e-9,
        "field conformance over a window the kernel intervened in every cycle is zero, not one",
    );

    drop(recorder);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Capture, in the cycle ───────────────────────────────────────────────────

mod capture {
    use super::*;
    use crate::capture::{Camera, Declared, RhalSensor};
    use crate::run::Attached;
    use skynet_capture::source::Source;

    /// Ten frames' worth of bytes, which is all a test pattern needs to be told apart.
    const FRAME_BYTES: usize = 10;

    fn body_with_camera() -> Fake {
        let mut body = Fake::new(2);
        body.camera = Some(("wrist".to_string(), FRAME_BYTES));
        body
    }

    fn declared(hz: f64) -> Declared {
        Declared {
            name: "wrist".to_string(),
            hz,
            buffers: 2,
            slot_bytes: 64,
        }
    }

    /// Turn a fresh loop at 100 Hz for `cycles` cycles with `camera` attached.
    ///
    /// The clock restarts at zero on every call, which the tests that call this twice rely on: the
    /// camera's own schedule is what decides whether a frame is due, and a schedule that has moved
    /// past the restarted clock reads nothing until it catches up.
    fn turn(camera: &mut Camera, body: &mut Fake, cycles: u64) -> Vec<crate::run::Turn> {
        let words = ring_words();
        let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
        let mut consumer = ring.consumer();
        let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
        let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
        (0..cycles)
            .map(|cycle| {
                host.step_observing(
                    cycle * 10_000_000,
                    body,
                    &mut consumer,
                    &limits,
                    None::<&Envelope<'_>>,
                    Attached {
                        observations: None,
                        camera: Some(&mut *camera),
                    },
                )
            })
            .collect()
    }

    /// The stream is read at the rate somebody declared, not at the loop's.
    ///
    /// §47.5's declaration is the only thing that sets it. A camera read on every cycle of a
    /// hundred-hertz loop is a hundred renders a second that nobody asked for, on the control
    /// thread.
    #[test]
    fn a_camera_is_read_at_its_declared_rate_and_not_the_loops() {
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(25.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None);

        let turns = turn(&mut camera, &mut body, 20);

        // Twenty cycles of ten milliseconds, and a frame due every forty: at 0, 40, 80, 120, 160.
        assert_eq!(camera.sensor().counts().reads, 5);
        assert_eq!(camera.sensor().counts().frames, 5);
        let captured: Vec<usize> = turns
            .iter()
            .map(|turn| turn.capture.as_ref().expect("a camera turn").captured)
            .collect();
        assert_eq!(
            captured,
            vec![1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]
        );
    }

    /// The command goes out before the camera is read.
    ///
    /// The write is the part of a cycle with a deadline. A frame read first would spend the
    /// period's budget before the one thing in it that cannot be late.
    #[test]
    fn a_cycle_writes_its_command_before_it_reads_the_camera() {
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None);

        turn(&mut camera, &mut body, 3);

        assert_eq!(
            body.frames_after,
            vec![1, 2, 3],
            "each frame was read after that cycle's command had been written"
        );
    }

    /// Every buffer the camera takes goes back in the same turn.
    ///
    /// §47.3's one hard rule. With no pool attached the frame has nowhere to go, and it is
    /// released anyway — a sensor that kept it would run dry after two frames and then look like a
    /// camera that stopped.
    #[test]
    fn every_buffer_the_camera_takes_comes_back() {
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None);

        turn(&mut camera, &mut body, 12);

        assert_eq!(camera.sensor().counts().frames, 12);
        assert_eq!(
            camera.sensor().outstanding(),
            0,
            "nothing is still lent out"
        );
        assert_eq!(
            camera.sensor().counts().starved,
            0,
            "so nothing ever starved"
        );
        assert_eq!(
            camera.counts().unpublished,
            12,
            "and every frame is counted as one nobody was offered, rather than as published"
        );
    }

    /// A frame the driver has already given is not a new frame.
    ///
    /// Reading a stream faster than it produces is a configuration somebody can choose, and a
    /// policy told the same image is fresh twice has been told something false about time. Here
    /// the body's own state read fails from the third cycle, so the fake's clock stops and its
    /// camera answers with the frame it already stamped.
    #[test]
    fn a_frame_the_driver_repeats_is_counted_and_not_handed_on() {
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None);

        let words = ring_words();
        let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
        let mut consumer = ring.consumer();
        let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
        let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
        let mut captured = Vec::new();
        for cycle in 0..3u64 {
            if cycle == 2 {
                body.read_fails = true;
            }
            let turn = host.step_observing(
                cycle * 10_000_000,
                &mut body,
                &mut consumer,
                &limits,
                None::<&Envelope<'_>>,
                Attached {
                    observations: None,
                    camera: Some(&mut camera),
                },
            );
            captured.push(turn.capture.map(|capture| capture.captured));
        }

        assert_eq!(camera.sensor().counts().reads, 3);
        assert_eq!(camera.sensor().counts().repeats, 1);
        assert_eq!(camera.sensor().counts().frames, 2);
        assert_eq!(
            captured,
            vec![Some(1), Some(1), Some(0)],
            "nothing was captured on the repeated read"
        );
    }

    /// A failed read is counted, kept with its code, and does not stop the cycle.
    #[test]
    fn a_failed_read_is_counted_and_the_cycle_still_turns() {
        let mut body = body_with_camera();
        body.camera_fails = true;
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None);

        let turns = turn(&mut camera, &mut body, 3);

        assert_eq!(turns.len(), 3, "every cycle still produced a record");
        assert_eq!(body.written.len(), 3, "and still wrote its command");
        assert_eq!(camera.sensor().counts().failures, 3);
        assert_eq!(
            camera.sensor().first_failure().map(|failure| failure.code),
            Some(-5)
        );
        assert_eq!(
            camera.sensor().outstanding(),
            0,
            "a failed read holds no buffer"
        );
    }

    /// Nothing is recorded, and the gap says exactly that.
    ///
    /// §47.4's recording side is declared empty on this build — there is no encoder — so every
    /// frame is an annotated gap from the first one on. What must not happen is the absence
    /// reading as a quiet stream.
    #[test]
    fn every_frame_is_an_annotated_gap_because_there_is_no_encoder() {
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None);

        turn(&mut camera, &mut body, 4);

        let gap = camera
            .close()
            .expect("the gap closes at the end of the run");
        assert_eq!((gap.from_seq, gap.to_seq, gap.frames), (0, 3, 4));
        assert_eq!(camera.close(), None, "and it is handed back once");
    }

    /// A declaration that would read nothing is refused, naming what to change.
    #[test]
    fn a_declaration_that_reads_nothing_is_refused_by_name() {
        for (hz, buffers, bytes, says) in [
            (0.0, 2, 64, "capture rate"),
            (f64::NAN, 2, 64, "capture rate"),
            (30.0, 0, 64, "at least one buffer"),
            (30.0, 2, 0, "at least one buffer"),
        ] {
            let refused = RhalSensor::new(
                0,
                &Declared {
                    name: "wrist".to_string(),
                    hz,
                    buffers,
                    slot_bytes: bytes,
                },
            )
            .expect_err("a stream that holds or reads nothing is not built");
            assert!(refused.contains(says), "{refused}");
        }
    }

    /// A frame reaches the pool a policy maps, bytes and all.
    ///
    /// The one thing the rest of this section only implies: the pixels a policy reads are the ones
    /// the driver rendered, carrying the sequence the camera assigned and the stamp the driver
    /// gave. Linux only, because the pool is a `memfd`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_frame_reaches_the_pool_a_policy_maps() {
        let geometry = skynet_frames::Geometry {
            slots: 2,
            slot_bytes: 64,
        };
        let pool = skynet_frames::Writer::create("wrist", geometry).expect("a pool");
        let [frames, loans] = pool.descriptors();
        let dup = |fd| skynet_ring_shm::pass::dup(fd).expect("dup");
        let reader =
            skynet_frames::Reader::adopt(dup(frames), dup(loans), geometry).expect("a reader");

        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, Some(pool));

        assert_eq!(
            reader.latest(),
            None,
            "nothing yet, and not a frame of zeros"
        );
        let turns = turn(&mut camera, &mut body, 3);
        assert_eq!(
            turns
                .iter()
                .map(|turn| turn.capture.as_ref().and_then(|capture| capture.published))
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(2)]
        );

        let latest = reader.latest().expect("a frame");
        let identity = reader.acquire(&latest).expect("a loan");
        assert_eq!(identity.sequence, 2);
        assert_eq!(identity.stream, "wrist");
        let mut pixels = Vec::new();
        reader.read(&latest, &mut pixels).expect("the bytes");
        assert_eq!(
            pixels,
            Fake::pattern(3, FRAME_BYTES),
            "the third frame the body rendered"
        );
        reader.release(&latest);
        assert_eq!(camera.counts().published, 3);
    }

    /// A policy holding every slot costs itself vision, and the camera keeps going.
    ///
    /// §47.6's `pool_exhausted`: dropped, counted, and capture is never stalled.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pool_with_every_slot_on_loan_drops_and_counts_and_capture_continues() {
        let geometry = skynet_frames::Geometry {
            slots: 1,
            slot_bytes: 64,
        };
        let pool = skynet_frames::Writer::create("wrist", geometry).expect("a pool");
        let [frames, loans] = pool.descriptors();
        let dup = |fd| skynet_ring_shm::pass::dup(fd).expect("dup");
        let reader =
            skynet_frames::Reader::adopt(dup(frames), dup(loans), geometry).expect("a reader");

        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, Some(pool));

        let words = ring_words();
        let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
        let mut consumer = ring.consumer();
        let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
        let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
        let mut step = |cycle: u64, camera: &mut Camera, body: &mut Fake| {
            host.step_observing(
                cycle * 10_000_000,
                body,
                &mut consumer,
                &limits,
                None::<&Envelope<'_>>,
                Attached {
                    observations: None,
                    camera: Some(camera),
                },
            )
            .capture
            .expect("a camera turn")
        };

        step(0, &mut camera, &mut body);
        let held = reader.latest().expect("the first frame");
        reader.acquire(&held).expect("and the policy holds it");

        for cycle in 1..=3 {
            let turned = step(cycle, &mut camera, &mut body);
            assert!(turned.refused.is_some(), "cycle {cycle}: {turned:?}");
        }
        assert_eq!(camera.counts().pool_dropped, 3);
        assert_eq!(camera.sensor().counts().frames, 4, "capture never stalled");
        assert_eq!(
            camera.sensor().outstanding(),
            0,
            "and never kept what it could not give"
        );

        reader.release(&held);
        let turned = step(4, &mut camera, &mut body);
        assert_eq!(
            turned.published,
            Some(4),
            "a released slot takes the next frame"
        );
    }

    // ── §89.2: capture yields to control ─────────────────────────────────────────

    /// A clock on which every read takes exactly F24's render: 18.2 ms between the two readings a
    /// turn takes. Its own static, so no other test's reads advance it.
    fn f24_clock() -> u64 {
        static NOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NOW.fetch_add(18_200_000, std::sync::atomic::Ordering::SeqCst)
    }

    /// A clock on which every read takes one millisecond.
    fn quick_clock() -> u64 {
        static NOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NOW.fetch_add(1_000_000, std::sync::atomic::Ordering::SeqCst)
    }

    /// A clock that has not moved since the last test that used it — for the late-anyway case,
    /// whose reads are timed like F24's.
    fn late_clock() -> u64 {
        static NOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NOW.fetch_add(18_200_000, std::sync::atomic::Ordering::SeqCst)
    }

    /// Turn `cycles` cycles at 100 Hz with every one a read (the camera is declared at the loop's
    /// rate), and settle each as though it took `cycle_ns` of a `period_ns` period. Returns what
    /// `settle` answered, per cycle.
    fn settling(
        camera: &mut Camera,
        body: &mut Fake,
        cycles: u64,
        cycle_ns: u64,
        period_ns: u64,
    ) -> Vec<Option<skynet_contracts::media::CapturePlacement>> {
        let words = ring_words();
        let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
        let mut consumer = ring.consumer();
        let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
        let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
        (0..cycles)
            .map(|cycle| {
                host.step_observing(
                    cycle * 10_000_000,
                    body,
                    &mut consumer,
                    &limits,
                    None::<&Envelope<'_>>,
                    Attached {
                        observations: None,
                        camera: Some(&mut *camera),
                    },
                );
                camera.settle(cycle_ns, period_ns)
            })
            .collect()
    }

    /// F24's stream, through the loop: an 18.2 ms read in a cycle that took 20.1 ms of a 20 ms
    /// period. The third read suspends it, the verdict is answered once, and nothing is read again —
    /// which is the whole of §89.2: the camera loses its frames and the period is the kernel's again.
    #[test]
    fn a_control_thread_read_that_keeps_costing_the_period_is_suspended_on_its_third() {
        use skynet_contracts::media::Placement;
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None).with_clock(f24_clock);

        let verdicts = settling(&mut camera, &mut body, 8, 20_100_000, 20_000_000);

        let answered: Vec<usize> = verdicts
            .iter()
            .enumerate()
            .filter(|(_, verdict)| verdict.is_some())
            .map(|(cycle, _)| cycle)
            .collect();
        assert_eq!(answered, vec![2], "once, on the cycle of the third read");
        let verdict = verdicts[2].expect("the suspending verdict");
        assert_eq!(verdict.suspended_at, Some(3));
        assert_eq!(verdict.misses, 3);
        assert_eq!(camera.placement(), Placement::Suspended);
        assert_eq!(
            camera.sensor().counts().reads,
            3,
            "a suspended stream is not read again"
        );
    }

    /// The control: the same reads in cycles whose other work already overran — 45 ms of a 20 ms
    /// period, 26.8 of it not the camera's. Not one read caused a miss, so nothing is suspended and
    /// every cycle keeps its read. Without the `rest_ns <= period_ns` half of the rule, this case and
    /// the one above would be the same case.
    #[test]
    fn a_read_in_a_cycle_that_was_late_anyway_is_not_held_against_it() {
        use skynet_contracts::media::Placement;
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None).with_clock(late_clock);

        let verdicts = settling(&mut camera, &mut body, 8, 45_000_000, 20_000_000);

        assert!(verdicts.iter().all(Option::is_none), "{verdicts:?}");
        assert_eq!(camera.placement(), Placement::ControlThread);
        assert_eq!(camera.sensor().counts().reads, 8);
    }

    /// A read that fits leaves the stream where it is, however many there are.
    #[test]
    fn a_read_that_fits_the_period_is_never_suspended() {
        use skynet_contracts::media::Placement;
        let mut body = body_with_camera();
        let sensor = RhalSensor::new(0, &declared(100.0)).expect("a stream");
        let mut camera = Camera::new(sensor, None).with_clock(quick_clock);

        let verdicts = settling(&mut camera, &mut body, 20, 3_000_000, 20_000_000);

        assert!(verdicts.iter().all(Option::is_none));
        assert_eq!(camera.placement(), Placement::ControlThread);
        assert_eq!(camera.sensor().counts().reads, 20);
    }
}

// ── §88: a skill reaches a command-level driver ──────────────────────────────

/// A body that takes `move_l pose speed`, with `pose` bounded and `speed` not.
///
/// `pose` bounded and `speed` not is deliberate and is half the point: §88.2 says an undeclared
/// bound does not refuse and the verdict says which values went unchecked, which is the same shape
/// §7.2 uses for a guard whose input nobody supplies.
fn motion_body() -> Vec<skynet_contracts::embodiment::EmbodimentMotion> {
    use skynet_contracts::embodiment::{EmbodimentMotion, MotionParameter};
    vec![EmbodimentMotion {
        name: "move_l".to_string(),
        motion: Some("move_l".to_string()),
        parameters: vec![
            MotionParameter {
                name: "pose".to_string(),
                unit: Some("m".to_string()),
                required: Some(true),
                min: Some(0.0),
                max: Some(1.0),
            },
            MotionParameter {
                name: "speed".to_string(),
                unit: Some("m/s".to_string()),
                required: Some(false),
                min: None,
                max: None,
            },
        ],
    }]
}

/// Everything §88.2 asks for, declared: a lease, a state that takes commands, and §4's token.
fn motion_authority() -> crate::motion::Authority {
    crate::motion::Authority {
        motions: motion_body(),
        state: "running".to_string(),
        lease_held_by: Some("skill:pick".to_string()),
        token: Some("vendor.pendant.motion".to_string()),
    }
}

/// One motion frame, ready to publish.
fn motion_frame(motion: &str, holder: &str, parameters: &[&str]) -> skynet_ring::Motion {
    let mut frame = skynet_ring::Motion {
        name: skynet_ring::Name::new(motion).expect("the motion name fits"),
        holder: skynet_ring::Name::new(holder).expect("the holder fits"),
        deadline_ms: 4000.0,
        parameters: [skynet_ring::Name::default(); skynet_ring::MAX_MOTION_PARAMETERS],
    };
    for (at, parameter) in parameters.iter().enumerate() {
        frame.parameters[at] = skynet_ring::Name::new(parameter).expect("the parameter fits");
    }
    frame
}

/// §88.1's whole claim, end to end: a motion posted on the ring a policy already writes reaches
/// the driver's `extension()`, with its arguments in the order §84.1 declared them.
///
/// The declaration order is what is being asserted rather than the delivery. A request whose
/// parameters arrive as `speed` then `pose` is handed over as `pose=… speed=…`, because a
/// program-level controller's motion is a call with positional arguments and the declaration order
/// is the only order anybody agreed on (§88.2).
#[test]
fn a_motion_on_the_ring_reaches_the_driver_in_declaration_order() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    ring.producer()
        // Reversed on the wire, so the assertion below is about the ordering rather than about
        // what happened to be published.
        .publish_motion(
            1_000_000,
            1,
            &motion_frame("move_l", "skill:pick", &["speed", "pose"]),
            &[0.1, 0.4],
        )
        .expect("the ring accepts it");

    let mut body = Fake::new(2).with_extension("vendor.pendant.motion");
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.authorise(motion_authority());

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    let motion = turn.motion.expect("the cycle had a motion in it");
    let verdict = motion.verdict.expect("the request was admitted on arrival");
    assert!(verdict.accepted, "{}", verdict.detail);
    assert_eq!(
        verdict.unbounded,
        vec!["speed".to_string()],
        "§88.2: the verdict says which values went unchecked rather than reporting a check it did \
         not perform",
    );
    assert_eq!(
        body.motions,
        vec!["move_l pose=0.4 speed=0.1".to_string()],
        "declaration order, not the order the request happened to carry",
    );
    assert_eq!(motion.reply.map(|(code, _)| code), Some(0));
}

/// §88.2's first refusal, and the control for the test above.
///
/// The same request, on a loop that has not been told who holds the body. Nothing reaches the
/// driver, and the reason is authority rather than content — the platform never read what was
/// asked for, because reading it would mean answering a question nobody was entitled to ask.
#[test]
fn a_motion_from_a_holder_that_does_not_hold_the_body_reaches_no_driver() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    ring.producer()
        .publish_motion(
            1_000_000,
            1,
            &motion_frame("move_l", "skill:somebody-else", &["pose"]),
            &[0.4],
        )
        .expect("the ring accepts it");

    let mut body = Fake::new(2).with_extension("vendor.pendant.motion");
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.authorise(motion_authority());

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    let motion = turn.motion.expect("the cycle had a motion in it");
    let verdict = motion.verdict.expect("the request was decided on arrival");
    assert!(!verdict.accepted);
    assert_eq!(
        verdict.refusal,
        Some(skynet_contracts::execute::MotionRefusalCode::NoLease)
    );
    assert!(body.motions.is_empty(), "nothing crossed §4");
    assert_eq!(host.motions().refused, 1);
    assert_eq!(host.motions().delivered, 0);
}

/// A loop that has been told nothing refuses everything, which is the default this design turns on.
#[test]
fn a_loop_that_was_never_authorised_refuses_every_motion() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    ring.producer()
        .publish_motion(
            1_000_000,
            1,
            &motion_frame("move_l", "skill:pick", &["pose"]),
            &[0.4],
        )
        .expect("the ring accepts it");

    let mut body = Fake::new(2).with_extension("vendor.pendant.motion");
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    let verdict = turn
        .motion
        .expect("the cycle had a motion in it")
        .verdict
        .expect("decided on arrival");
    assert_eq!(
        verdict.refusal,
        Some(skynet_contracts::execute::MotionRefusalCode::NoLease),
        "the fail-closed default: nothing holds a body this process was told nothing about",
    );
    assert!(body.motions.is_empty());
}

/// §88.3's deadline is what replaces §7.1's watchdog, and it has to fire on a silent cycle.
///
/// The request is published once and nothing is published after it. A motion takes seconds, during
/// which a correctly behaving policy produces nothing — so the bound is asked on every cycle rather
/// than on the ones that carry a request, and the control is the cycle before the deadline, where
/// the same loop finds the motion running and aborts nothing.
#[test]
fn a_motion_held_past_its_deadline_is_aborted_on_a_cycle_with_no_request_on_it() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    let mut frame = motion_frame("move_l", "skill:pick", &["pose"]);
    frame.deadline_ms = 50.0;
    ring.producer()
        .publish_motion(1_000_000, 1, &frame, &[0.4])
        .expect("the ring accepts it");

    let mut body = Fake::new(2).with_extension("vendor.pendant.motion");
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.authorise(motion_authority());

    // Arrival, at 2 ms.
    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    assert!(
        turn.motion
            .expect("arrival")
            .verdict
            .expect("decided")
            .accepted
    );

    // 40 ms: inside the deadline, and the loop is reading the same slot it read before.
    let turn = host.step(
        40_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    let running = turn.motion.expect("a motion in flight is still a motion");
    assert!(!running.overdue, "{}", running.detail);
    assert!(running.verdict.is_none(), "not a new request");
    assert_eq!(body.stops, 0, "nothing was aborted");

    // 60 ms: past it.
    let turn = host.step(
        60_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    let overdue = turn.motion.expect("the deadline is asked every cycle");
    assert!(overdue.overdue, "{}", overdue.detail);
    assert_eq!(body.stops, 1, "§88.3: the driver is asked to abort");
    assert_eq!(host.motions().overdue, 1);
    assert_eq!(
        body.motions.len(),
        1,
        "and the controller was asked exactly once, on the cycle the request arrived",
    );
}

/// A motion is not a joint command, and the loop must not read it as one (§84.4).
///
/// The frame's `values` are `pose` and `speed`; read as a command they are two channels of a
/// two-channel body. The mask is what stops it, and this asserts the loop never even got that far:
/// the write is a hold, and `desired` — which is what the envelope guard checks — is empty.
#[test]
fn a_motion_frame_is_never_read_as_a_joint_command() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    ring.producer()
        .publish_motion(
            1_000_000,
            1,
            &motion_frame("move_l", "skill:pick", &["pose", "speed"]),
            &[0.4, 0.1],
        )
        .expect("the ring accepts it");

    let mut body = Fake::new(2).with_extension("vendor.pendant.motion");
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.authorise(motion_authority());

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert!(
        turn.cycle.desired.iter().all(|value| value.is_nan()),
        "a motion's parameters are not a policy's desired joint values — and `NaN` rather than zero \
         is what the record spells *no value here* (cycle.rs), so a zero would be the claim this \
         asserts against",
    );
    assert!(
        body.written
            .last()
            .expect("a cycle always writes")
            .mask
            .iter()
            .all(|entry| *entry == 0),
        "and what reached the body is a hold",
    );
    assert!((body.position[0] - 0.0).abs() < 1e-6, "no joint moved");
}

/// An admitted motion with nowhere to go is recorded as undelivered rather than as refused.
///
/// Those are different facts with different remedies — one is fixed at the policy and one at the
/// descriptor — and a report that collapsed them would send an integrator to the wrong file.
#[test]
fn an_admitted_motion_with_no_route_is_undelivered_rather_than_refused() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    ring.producer()
        .publish_motion(
            1_000_000,
            1,
            &motion_frame("move_l", "skill:pick", &["pose"]),
            &[0.4],
        )
        .expect("the ring accepts it");

    // The body has no escape hatch at all.
    let mut body = Fake::new(2);
    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(1.0), &[]);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);
    host.authorise(motion_authority());

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    let motion = turn.motion.expect("the cycle had a motion in it");
    assert!(
        motion.verdict.expect("decided").accepted,
        "the request itself was sound",
    );
    assert!(motion.delivered.is_none(), "and nothing carried it");
    assert_eq!(host.motions().refused, 0, "the policy did nothing wrong");
    assert_eq!(host.motions().delivered, 0);
    assert!(
        motion.detail.contains("implements no `extension`"),
        "the sentence names the fix: {}",
        motion.detail
    );
}

// ── §35.3: a simulated body's time is the kernel's time (DRY-COVERAGE-PLAN.md F70) ───────────────

/// An hour of `CLOCK_MONOTONIC_RAW`: what the machine's clock reads on a robot that has been up for
/// one, and the number a simulator's stamps — which start at zero — were aged against before F70.
const AN_HOUR_NS: u64 = 3_600_000_000_000;

/// The watchdog window the §35.3 cases declare, in milliseconds.
const WATCHDOG_MS: f64 = 50.0;

fn watchdog_envelope() -> Envelope<'static> {
    Envelope {
        joint_velocity_max_rad_s: None,
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        watchdog_ms: Some(WATCHDOG_MS),
        estop: Some(true),
        source: Some("the §35.3 test"),
        basis: None,
    }
}

/// A command stamped by a policy on the simulator's clock, one period after the body's last stamp.
fn simulated_command(stamp_ns: u64, seq: u64) -> Command {
    let mut command = Command {
        stamp_ns,
        seq,
        mode: mode::POSITION,
        channels: 2,
        ..Command::default()
    };
    command.values[0] = 0.1;
    command.mask[0] = 1;
    command
}

/// On a simulated body the kernel is asked at the body's own time, and a command stamped on that
/// time is current — however far along the machine's clock is (§35.3).
///
/// `drivers/mujoco` stamps a state with the simulation's time, which starts at zero; the machine's
/// clock is an hour in. The loop took the second as the kernel's `now` whatever the body declared,
/// so a command a policy stamped on the simulation's clock was an hour old to the watchdog. Before
/// F70 nothing produced such a command — the policy read the machine's clock too — and the
/// half-wired arrangement below is what either side alone would have done.
#[test]
fn a_simulated_body_is_the_kernels_clock() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let envelope = watchdog_envelope();
    let limits = limits(&CEILINGS, &[Guard::Watchdog], Some(0.1), &[]);
    // Ten milliseconds of simulation per read, while the machine's clock moves twenty: a
    // simulator at half real time, which §35.3 calls ordinary.
    let mut body = simulated_fake(2, 10_000_000);
    let mut host = Host::new(&CHANNELS, TimeBase::Simulated);

    let mut seq = 0;
    for cycle in 0..20u64 {
        // The policy stamps with the newest time the kernel published — the body's last stamp.
        seq += 1;
        producer
            .publish(&simulated_command(body.simulated_ns, seq))
            .expect("the ring accepts it");
        let turn = host.step(
            AN_HOUR_NS + cycle * 20_000_000,
            &mut body,
            &mut consumer,
            &limits,
            Some(&envelope),
        );
        assert_eq!(
            turn.now_ns, body.simulated_ns,
            "cycle {cycle}: the kernel's time is the stamp of the state it read",
        );
        assert_eq!(turn.cycle.state_stamp_ns, turn.now_ns);
        assert!(
            !turn.cycle.tripped.contains(&Guard::Watchdog),
            "cycle {cycle}: a command one simulated period old is current, and the watchdog \
             tripped on it",
        );
    }
    assert_eq!(host.summary().interventions, 0);
    assert_eq!(host.time_base(), TimeBase::Simulated);
    assert_eq!(
        host.now_ns(AN_HOUR_NS * 2),
        body.simulated_ns,
        "and between cycles the kernel's time is still the body's, whatever the machine reads",
    );
}

/// The control: the same body and the same commands, with the loop keeping the machine's clock —
/// which is what `skynetk` did on every body until F70 — trips the watchdog on every cycle.
#[test]
fn a_simulated_body_on_the_machines_clock_trips_the_watchdog() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let envelope = watchdog_envelope();
    let limits = limits(&CEILINGS, &[Guard::Watchdog], Some(0.1), &[]);
    let mut body = simulated_fake(2, 10_000_000);
    let mut host = Host::new(&CHANNELS, TimeBase::Monotonic);

    producer
        .publish(&simulated_command(body.simulated_ns, 1))
        .expect("the ring accepts it");
    let turn = host.step(
        AN_HOUR_NS,
        &mut body,
        &mut consumer,
        &limits,
        Some(&envelope),
    );
    assert_eq!(
        turn.now_ns, AN_HOUR_NS,
        "the machine's clock, not the body's"
    );
    assert!(
        turn.cycle.tripped.contains(&Guard::Watchdog),
        "a command stamped on the simulation's clock is an hour old on the machine's",
    );
}

/// A cycle whose read failed keeps the last simulated time rather than inventing one.
///
/// A simulator that did not step has not moved its clock. Advancing the kernel's time by the
/// machine's elapsed period would be this process deciding how fast the simulation runs; falling
/// back to the machine's clock would put an hour-sized jump into the record between two cycles.
#[test]
fn a_failed_read_on_a_simulated_body_keeps_its_last_time() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring of eight slots");
    let mut consumer = ring.consumer();

    let limits = limits(&CEILINGS, &[Guard::Envelope], Some(0.1), &[]);
    let mut body = simulated_fake(2, 10_000_000);
    let mut host = Host::new(&CHANNELS, TimeBase::Simulated);

    let first = host.step(
        AN_HOUR_NS,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    assert_eq!(first.now_ns, 10_000_000);

    body.read_fails = true;
    let failed = host.step(
        AN_HOUR_NS + 20_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    assert!(failed.read_error.is_some(), "the read did fail");
    assert_eq!(
        failed.now_ns, 10_000_000,
        "the simulation did not step, so its time did not move",
    );
    assert_eq!(
        failed.cycle.state_stamp_ns, 10_000_000,
        "and the record's fallback stamp is the kernel's time, on the body's clock",
    );

    body.read_fails = false;
    let resumed = host.step(
        AN_HOUR_NS + 40_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    assert_eq!(
        resumed.now_ns, 20_000_000,
        "one step later, one period later"
    );
}

/// The time base comes off the body's own descriptor, and absence is a real body (§35.3).
///
/// *"A real robot should not have to declare what it is not"*: a descriptor with no `simulator`
/// key is `Monotonic`. The case that matters is the other one — before F70 nothing read the flag
/// here at all, so `drivers/mujoco` ran on the machine's clock and sealed its windows as field runs.
#[test]
fn the_time_base_is_read_off_the_descriptor() {
    let decode = |text: &str| {
        skynet_contracts::json::descriptor_from_json(
            &skynet_json::Json::parse(text).expect("the fixture is JSON"),
        )
        .expect("the fixture decodes")
    };
    assert_eq!(
        crate::run::time_base_of(&decode(
            r#"{"robot_slug": "sim", "base": "fixed", "dof": 0, "simulator": true}"#
        )),
        TimeBase::Simulated,
    );
    assert_eq!(
        crate::run::time_base_of(&decode(
            r#"{"robot_slug": "arm", "base": "fixed", "dof": 0, "simulator": false}"#
        )),
        TimeBase::Monotonic,
    );
    assert_eq!(
        crate::run::time_base_of(&decode(
            r#"{"robot_slug": "arm", "base": "fixed", "dof": 0}"#
        )),
        TimeBase::Monotonic,
        "a descriptor that does not mention it is a real body",
    );
}
