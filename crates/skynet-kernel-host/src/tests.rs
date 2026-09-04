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

const CHANNELS: [&str; 2] = ["shoulder", "elbow"];
const CEILINGS: [(&str, f64); 2] = [("shoulder", 1.0), ("elbow", 1.0)];

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
        active_guards: active,
        unavailable_guards: FixedVec::new(),
        source: Some("test"),
        notices: FixedVec::new(),
        grants,
        frames: None,
        workspace: &[],
        self_collision_margin_m: None,
    }
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
    let mut host = Host::new(&CHANNELS);

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
            .all(|(_, _, mask)| mask.iter().all(|entry| *entry == 0)),
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
    let mut host = Host::new(&CHANNELS);

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
        "the record carries what the policy asked for, not only what was sent",
    );
    assert!((body.position[0] - 0.4).abs() < 1e-6, "and the body moved");
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
    let mut host = Host::new(&CHANNELS);

    let turn = host.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert_eq!(
        turn.cycle.desired.len(),
        1,
        "one channel was commanded, so one value was desired",
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
    let mut host = Host::new(&CHANNELS);

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
    let mut host = Host::new(&CHANNELS);

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
    let mut host = Host::new(&CHANNELS);

    let turn = host.step(
        1_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert_eq!(turn.cycle.rhal_result, -13, "surfaced verbatim, per §24");
    assert_eq!(host.summary().write_failures, 1);
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
        &observed(&zeros),
        None,
        &decision,
        0,
    );

    assert!(cycle.nearest_person_m.is_nan());
    assert!(
        cycle.desired.is_empty(),
        "nothing was posted, so nothing was desired — not a row of zeros",
    );
    assert_eq!(cycle.mask.len(), CHANNELS.len());
    assert_eq!(cycle.clamped.len(), CHANNELS.len());
    assert!(!cycle.intervened(), "no guard tripped");
    assert!(MAX_CHANNELS >= CHANNELS.len());
}
