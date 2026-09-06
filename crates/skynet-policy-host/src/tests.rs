//! Tests for the producer.
//!
//! The cases worth having are the ones about the **sequence**, because §24 makes a repeated
//! sequence the definition of a stalled producer and this crate is the only thing that decides when
//! that number moves. Everything else here is downstream of getting that right.
//!
//! The last one is different in kind: it puts this producer and `skynet-kernel-host`'s consumer on
//! one ring and drives a command from a backend, through the eight guards, into a driver. That is
//! §25's whole path, and until this crate existed no test anywhere could write it — the kernel
//! host's own suite had to hand-build a `Command` and post it, which exercises the ring and
//! declares nothing about whether a policy runtime would produce a command a kernel can use.

use skynet_kernel::fixed::FixedVec;
use skynet_kernel::{Enforcement, Envelope, Guard, ResolvedLimits, Tier};
use skynet_kernel_host::driver::Fake;
use skynet_kernel_host::run::Host;
use skynet_ring::{mode, words_for, Ring, MAX_CHANNELS};
use std::sync::atomic::AtomicU32;

use crate::backend::{Approach, Backend, Body, Commanding, Hold, Intent, Trajectory};
use crate::run::{PolicyHost, Refusal, StepClock, Tick};

const SLOTS: usize = 8;

fn ring_words() -> Vec<AtomicU32> {
    (0..words_for(SLOTS)).map(|_| AtomicU32::new(0)).collect()
}

fn lease(channels: usize) -> Commanding {
    Commanding {
        channels,
        observation_budget_ms: None,
        mode: mode::POSITION,
    }
}

/// A clock that does not move, so a turn never looks like an overrun.
fn still() -> StepClock {
    StepClock {
        now_ns: 1_000_000,
        step_ns: 0,
    }
}

/// A backend that returns whatever it is handed, one intent per tick.
struct Scripted(Vec<Intent>);

impl Backend for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn decide(&mut self, tick: u64, _now_ns: u64, _body: Body<'_>) -> Intent {
        self.0.get(tick as usize).copied().unwrap_or(Intent::Silent)
    }
}

// ── the sequence ─────────────────────────────────────────────────────────────

/// The first command carries one, not zero.
///
/// Zero is what an unwritten slot already holds, so a first command carrying it would be
/// indistinguishable from a ring nobody has published to — and `Reading::Empty` exists precisely to
/// be told apart from a real first command.
#[test]
fn the_first_command_carries_one() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(Hold, still(), lease(2), 10_000_000);

    assert_eq!(host.sequence(), 0, "nothing published yet");
    assert_eq!(host.turn(&mut producer, None), Tick::Held(1));
}

/// A hold advances the sequence. A silence does not. This is the whole design.
#[test]
fn a_hold_advances_the_sequence_and_a_silence_does_not() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(
        Scripted(vec![Intent::Hold, Intent::Silent, Intent::Hold]),
        still(),
        lease(2),
        10_000_000,
    );

    assert_eq!(host.turn(&mut producer, None), Tick::Held(1));
    assert_eq!(
        host.turn(&mut producer, None),
        Tick::Silent,
        "a policy that has stopped producing publishes nothing",
    );
    assert_eq!(
        host.sequence(),
        1,
        "and the number the watchdog reads has not moved, which is what makes it a stalled producer",
    );
    assert_eq!(
        host.turn(&mut producer, None),
        Tick::Held(2),
        "a producer that comes back carries the next sequence, not the one it skipped",
    );
}

/// A hold and a silence are different events, and the ring can tell them apart.
///
/// The one a reader would get wrong: after a silence the ring still holds the previous command, so
/// *reading* the ring shows the same command twice. What distinguishes them is the sequence inside
/// it, which is exactly what §7.1's watchdog guard compares.
#[test]
fn a_silence_leaves_the_previous_command_in_the_ring_with_its_old_sequence() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    let mut host = PolicyHost::new(
        Scripted(vec![Intent::apply(&[0.5, -0.5]), Intent::Silent]),
        still(),
        lease(2),
        10_000_000,
    );

    assert_eq!(host.turn(&mut producer, None), Tick::Published(1));
    let mut first = skynet_ring::Command::default();
    consumer.read(&mut first);
    assert_eq!(first.seq, 1);

    assert_eq!(host.turn(&mut producer, None), Tick::Silent);
    let mut second = skynet_ring::Command::default();
    consumer.read(&mut second);
    assert_eq!(
        second.seq, 1,
        "the same sequence, because nothing was published over it",
    );
    assert!(
        (second.values[0] - 0.5).abs() < 1e-6,
        "and the same command, which is why the sequence is the only thing that can tell",
    );
}

// ── the lease ────────────────────────────────────────────────────────────────

/// A channel beyond the lease is a refusal, not a clamp.
#[test]
fn commanding_past_the_lease_is_refused_rather_than_narrowed() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(
        Scripted(vec![Intent::apply(&[0.1, 0.2, 0.3])]),
        still(),
        lease(2),
        10_000_000,
    );

    assert_eq!(
        host.turn(&mut producer, None),
        Tick::Refused(Refusal::ChannelsExceeded),
    );
    assert_eq!(
        host.sequence(),
        0,
        "a refused command did not reach the ring, so the sequence did not move",
    );
}

/// A value left in a scratch buffer past the lease is not a command.
///
/// The check is over the mask rather than the values on purpose: §24 asks a producer to keep one
/// full-width buffer and reuse it, so a non-zero float above the lease is ordinary debris and
/// refusing it would refuse the thing the specification asks for.
#[test]
fn a_masked_out_value_beyond_the_lease_is_debris_and_not_a_command() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();

    let mut values = [0.0; MAX_CHANNELS];
    let mut mask = [0u8; MAX_CHANNELS];
    values[0] = 0.4;
    mask[0] = 1;
    values[9] = 99.0; // never masked in

    let mut host = PolicyHost::new(
        Scripted(vec![Intent::Command { values, mask }]),
        still(),
        lease(2),
        10_000_000,
    );
    assert_eq!(host.turn(&mut producer, None), Tick::Published(1));
}

/// A lease this ring cannot carry is caught before any backend runs.
#[test]
fn an_impossible_lease_refuses_every_turn() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(
        Hold,
        still(),
        Commanding {
            channels: MAX_CHANNELS + 1,
            observation_budget_ms: None,
            mode: mode::POSITION,
        },
        10_000_000,
    );

    assert_eq!(
        host.turn(&mut producer, None),
        Tick::Refused(Refusal::LeaseInvalid)
    );
    assert_eq!(host.sequence(), 0);
}

/// `Commanding::none()` commands nothing rather than the first joint.
#[test]
fn the_default_lease_commands_no_joint() {
    assert_eq!(Commanding::none().channels, 0);
    assert!(Commanding::none().valid());
}

// ── the backends ─────────────────────────────────────────────────────────────

/// A finished trajectory holds. It does not go silent, which would stop the robot with a fault.
#[test]
fn a_finished_trajectory_holds_rather_than_stopping_the_robot() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(
        Trajectory::from_setpoints(&[(0, &[0.1, 0.2][..])]),
        still(),
        lease(2),
        10_000_000,
    );

    assert_eq!(host.turn(&mut producer, None), Tick::Published(1));
    assert_eq!(host.turn(&mut producer, None), Tick::Held(2));
    assert_eq!(host.turn(&mut producer, None), Tick::Held(3));
    assert!(host.backend().finished());
    assert_eq!(
        host.summary().silent,
        0,
        "a policy that finished successfully is not a policy that crashed",
    );
}

/// A row that is not due yet holds, and the row still plays when its tick arrives.
#[test]
fn a_row_waits_for_its_tick_without_being_skipped() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    let mut host = PolicyHost::new(
        Trajectory::from_setpoints(&[(2, &[0.7][..])]),
        still(),
        lease(1),
        10_000_000,
    );

    assert_eq!(host.turn(&mut producer, None), Tick::Held(1));
    assert_eq!(host.turn(&mut producer, None), Tick::Held(2));
    assert_eq!(host.turn(&mut producer, None), Tick::Published(3));

    let mut command = skynet_ring::Command::default();
    consumer.read(&mut command);
    assert!((command.values[0] - 0.7).abs() < 1e-6);
}

/// Hold publishes an all-zero mask, which is what holds a torque-controlled joint.
#[test]
fn a_hold_masks_every_channel_out_rather_than_commanding_zero() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();
    let mut host = PolicyHost::new(Hold, still(), lease(3), 10_000_000);

    assert_eq!(host.turn(&mut producer, None), Tick::Held(1));
    let mut command = skynet_ring::Command::default();
    consumer.read(&mut command);
    assert!(
        command.mask.iter().all(|applied| *applied == 0),
        "a zero value on a torque joint goes limp; a zero mask holds position",
    );
    assert_eq!(command.channels, 3, "the lease's width, not the mask's");
}

// ── the clock ────────────────────────────────────────────────────────────────

/// The stamp is when the command was produced, not when it was published.
#[test]
fn the_stamp_is_taken_before_the_backend_runs() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    // Every reading advances by a full period, so the reading taken after the backend is a period
    // later than the one taken before it.
    let clock = StepClock {
        now_ns: 5_000_000,
        step_ns: 10_000_000,
    };
    let mut host = PolicyHost::new(Scripted(vec![Intent::apply(&[0.2])]), clock, lease(1), 1);

    assert_eq!(host.turn(&mut producer, None), Tick::Published(1));
    let mut command = skynet_ring::Command::default();
    consumer.read(&mut command);
    assert_eq!(
        command.stamp_ns, 5_000_000,
        "the first reading, taken before the backend was asked",
    );
    assert_eq!(
        host.summary().overruns,
        1,
        "and the turn took longer than its period, which is counted rather than acted on",
    );
}

// ── §25, end to end ──────────────────────────────────────────────────────────

/// A command produced by a policy backend reaches a driver through the real kernel.
///
/// Both halves of §25 in one process: this crate's producer, `skynet-kernel-host`'s consumer, one
/// ring, the eight guards, a body. Nothing here builds a `Command` by hand — the values written to
/// the driver came out of a [`Trajectory`] and crossed the wire.
///
/// The process split is not modelled and does not need to be. What two processes give is fault
/// isolation and a sandbox; what they change about the *protocol* is nothing, and the protocol is
/// what has never been exercised from both ends.
#[test]
fn the_whole_control_path_in_one_process() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let channels: [&str; 2] = ["shoulder", "elbow"];
    let per_channel: [(&str, f64); 2] = [("shoulder", 1.0), ("elbow", 1.0)];
    let mut active: FixedVec<Guard, 8> = FixedVec::new();
    active.push(Guard::Envelope);
    let limits = ResolvedLimits {
        tier: Tier::Host,
        enforcement: Enforcement::Enforcing,
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        min_control_hz: Some(100.0),
        max_delta_per_cycle: Some(1.0),
        proximity: None,
        per_channel: &per_channel,
        active_guards: active,
        unavailable_guards: FixedVec::new(),
        source: Some("the end-to-end test"),
        notices: FixedVec::new(),
        grants: &[],
        frames: None,
        workspace: &[],
        self_collision_margin_m: None,
    };

    let mut policy = PolicyHost::new(
        Trajectory::from_setpoints(&[(0, &[0.4, -0.3][..])]),
        still(),
        lease(2),
        10_000_000,
    );
    let mut kernel = Host::new(&channels);
    let mut body = Fake::new(2);

    assert_eq!(
        policy.turn(&mut producer, None),
        Tick::Published(1),
        "the policy commanded two joints",
    );

    let turn = kernel.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert_eq!(
        turn.cycle.command_seq, 1,
        "the kernel read the sequence this crate assigned",
    );
    assert_eq!(turn.decision.mask[0], 1, "and the channel was commanded");
    assert!((turn.decision.values[0] - 0.4).abs() < 1e-6);
    assert!((turn.decision.values[1] - (-0.3)).abs() < 1e-6);
    assert!(
        (body.position[0] - 0.4).abs() < 1e-6,
        "and the body moved to where a backend asked it to",
    );

    // The second turn is the one that says something the kernel host's own suite cannot: a policy
    // that has finished holds, the sequence still advances, and the robot is not stopped for it.
    assert_eq!(policy.turn(&mut producer, None), Tick::Held(2));
    let held = kernel.step(
        12_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );
    assert_eq!(
        held.cycle.command_seq, 2,
        "a live producer, one cycle later"
    );
    assert!(
        held.decision.mask.iter().all(|applied| *applied == 0),
        "asking for nothing, which holds the joint where the trajectory left it",
    );
    assert!(
        (body.position[0] - 0.4).abs() < 1e-6,
        "so the body stayed where it was rather than returning to zero",
    );
}

/// A policy asks for more than its envelope allows, and a guard trips on it.
///
/// §73's interface row has said, in every revision since the guard-trip column got its source, that
/// what it does not have is *"a robot that trips a guard"* — and it could not have had one, because
/// tripping a guard requires a command, and until this crate nothing produced one outside a test
/// that wrote the command by hand.
///
/// This is the first trip in this repository whose cause is a **policy**: a trajectory asking a
/// joint to go to 5.0 radians against a per-channel ceiling of 1.0. The envelope guard clamps it,
/// `Cycle::intervened` becomes true, and that boolean is the numerator §15's `field_conformance` is
/// a ratio of. The figure is still not *reported* by anything — that needs a window and a robot —
/// but it is no longer a ratio with nothing to count.
#[test]
fn a_policy_that_overruns_its_envelope_trips_a_guard() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut consumer = Ring::open(&words).expect("the same ring").consumer();

    let channels: [&str; 1] = ["shoulder"];
    let per_channel: [(&str, f64); 1] = [("shoulder", 1.0)];
    let mut active: FixedVec<Guard, 8> = FixedVec::new();
    active.push(Guard::Envelope);
    let limits = ResolvedLimits {
        tier: Tier::Host,
        enforcement: Enforcement::Enforcing,
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        min_control_hz: Some(100.0),
        max_delta_per_cycle: None,
        proximity: None,
        per_channel: &per_channel,
        active_guards: active,
        unavailable_guards: FixedVec::new(),
        source: Some("the guard-trip test"),
        notices: FixedVec::new(),
        grants: &[],
        frames: None,
        workspace: &[],
        self_collision_margin_m: None,
    };

    let mut policy = PolicyHost::new(
        Trajectory::from_setpoints(&[(0, &[5.0][..])]),
        still(),
        lease(1),
        10_000_000,
    );
    let mut kernel = Host::new(&channels);
    let mut body = Fake::new(1);

    assert_eq!(
        policy.turn(&mut producer, None),
        Tick::Published(1),
        "the runtime published what the policy asked for, unnarrowed — clamping is the kernel's",
    );

    let turn = kernel.step(
        2_000_000,
        &mut body,
        &mut consumer,
        &limits,
        None::<&Envelope<'_>>,
    );

    assert!(
        turn.cycle.tripped.contains(&Guard::Envelope),
        "the envelope guard tripped on a policy's command",
    );
    assert!(
        turn.cycle.intervened(),
        "which is the boolean field_conformance counts",
    );
    assert!(
        (turn.decision.values[0] - 1.0).abs() < 1e-6,
        "clamped to the ceiling rather than refused outright",
    );
    assert!(
        (body.position[0] - 1.0).abs() < 1e-6,
        "and the body went to the ceiling, not to where the policy asked",
    );
    assert!(
        (turn.cycle.desired[0] - 5.0).abs() < 1e-6,
        "while the record keeps what was asked for, which is what makes the trip investigable",
    );
}

// ── the observation ring ─────────────────────────────────────────────────────

fn obs_words() -> Vec<AtomicU32> {
    (0..skynet_obs::words_for(4))
        .map(|_| AtomicU32::new(0))
        .collect()
}

fn body_at(joints: usize, stamp_ns: u64, position: &[f32]) -> skynet_obs::Observation {
    let mut observation = skynet_obs::Observation {
        stamp_ns,
        seq: 1,
        joints,
        ..skynet_obs::Observation::default()
    };
    for (index, value) in position.iter().enumerate() {
        observation.position[index] = *value;
    }
    observation
}

/// A backend that records what body it was handed, so the wiring can be asserted on.
struct Watcher {
    seen: Vec<String>,
}

impl Backend for Watcher {
    fn name(&self) -> &'static str {
        "watcher"
    }

    fn decide(&mut self, _tick: u64, _now_ns: u64, body: Body<'_>) -> Intent {
        self.seen.push(match body {
            Body::Fresh {
                observation,
                budgeted,
                ..
            } => format!("fresh({}, budgeted={budgeted})", observation.joints),
            Body::Empty => "empty".to_string(),
            Body::Torn => "torn".to_string(),
            Body::Detached => "detached".to_string(),
        });
        Intent::Hold
    }
}

/// No ring attached is `Detached`, not `Empty`. They are different situations.
#[test]
fn an_open_loop_deployment_is_told_it_is_detached() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(Watcher { seen: Vec::new() }, still(), lease(1), 10_000_000);

    host.turn(&mut producer, None);
    assert_eq!(host.backend().seen, ["detached"]);
}

/// A ring nobody has published to is `Empty`, which is normal at start.
#[test]
fn a_policy_that_started_first_is_told_the_ring_is_empty() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();

    let region = obs_words();
    skynet_obs::Ring::create(&region, 4).expect("an observation ring");
    let mut observations = skynet_obs::Ring::open(&region)
        .expect("the same ring")
        .consumer();

    let mut host = PolicyHost::new(Watcher { seen: Vec::new() }, still(), lease(1), 10_000_000);
    host.turn(&mut producer, Some(&mut observations));
    assert_eq!(host.backend().seen, ["empty"]);
}

/// A reading older than the lease's budget is a hold the backend never sees.
///
/// §25.1: *"Exceeding it is a `hold`, not a silently older frame."* The backend is not consulted,
/// so it cannot decide otherwise — and the hold is counted separately from a hold the policy chose,
/// because "the robot stopped moving" has two very different answers.
#[test]
fn a_reading_past_its_budget_holds_without_asking_the_backend() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();

    let region = obs_words();
    let obs_ring = skynet_obs::Ring::create(&region, 4).expect("an observation ring");
    obs_ring
        .producer()
        // Stamped at zero; the clock reads 1 ms, so the reading is 1 ms old.
        .publish(&body_at(1, 0, &[0.25]))
        .expect("published");
    let mut observations = skynet_obs::Ring::open(&region)
        .expect("the same ring")
        .consumer();

    let mut host = PolicyHost::new(
        Watcher { seen: Vec::new() },
        still(),
        Commanding {
            channels: 1,
            observation_budget_ms: Some(0.5),
            mode: mode::POSITION,
        },
        10_000_000,
    );

    assert_eq!(
        host.turn(&mut producer, Some(&mut observations)),
        Tick::Held(1)
    );
    assert!(
        host.backend().seen.is_empty(),
        "the backend was never asked, which is what makes the rule the runtime's",
    );
    assert_eq!(host.summary().stale_holds, 1);
    assert_eq!(
        host.summary().held,
        1,
        "and it is still a hold on the wire: the sequence advanced, so the watchdog sees a live \
         producer rather than a stalled one",
    );
}

/// A reading with no budget declared is passed through, and counted as unbudgeted.
#[test]
fn a_reading_with_no_budget_is_passed_through_and_counted() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();

    let region = obs_words();
    let obs_ring = skynet_obs::Ring::create(&region, 4).expect("an observation ring");
    obs_ring
        .producer()
        .publish(&body_at(2, 0, &[0.25, 0.5]))
        .expect("published");
    let mut observations = skynet_obs::Ring::open(&region)
        .expect("the same ring")
        .consumer();

    let mut host = PolicyHost::new(Watcher { seen: Vec::new() }, still(), lease(2), 10_000_000);
    host.turn(&mut producer, Some(&mut observations));

    assert_eq!(host.backend().seen, ["fresh(2, budgeted=false)"]);
    assert_eq!(
        host.summary().unbudgeted,
        1,
        "nothing in this system could say whether that body was current",
    );
}

/// The loop closes: what the policy commands depends on what the robot did.
///
/// The first backend in this repository whose command on cycle *n* is a function of cycle *n − 1*.
/// Everything before it — every trajectory, every hand-built `Command` in every test — was open
/// loop, which is why §25.1's assembler had never been needed by anything.
#[test]
fn the_loop_closes_and_the_command_follows_the_body() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut commands = Ring::open(&words).expect("the same ring").consumer();

    let region = obs_words();
    let obs_ring = skynet_obs::Ring::create(&region, 4).expect("an observation ring");
    let mut bodies = obs_ring.producer();
    let mut observations = skynet_obs::Ring::open(&region)
        .expect("the same ring")
        .consumer();

    // Halve the remaining distance to 1.0 each cycle.
    let mut host = PolicyHost::new(Approach::toward(&[1.0], 0.5), still(), lease(1), 10_000_000);

    let mut measured = 0.0f32;
    let mut commanded = Vec::new();
    for cycle in 0..4 {
        bodies
            .publish(&body_at(1, 0, &[measured]))
            .expect("the body is published");
        assert_eq!(
            host.turn(&mut producer, Some(&mut observations)),
            Tick::Published(cycle + 1),
        );
        let mut command = skynet_ring::Command::default();
        commands.read(&mut command);
        commanded.push(command.values[0]);
        // The body arrives where it was told, which is what closes the loop for this test.
        measured = command.values[0];
    }

    assert!((commanded[0] - 0.5).abs() < 1e-6, "half of the way from 0");
    assert!(
        (commanded[1] - 0.75).abs() < 1e-6,
        "then half of what is left"
    );
    assert!((commanded[2] - 0.875).abs() < 1e-6);
    assert!((commanded[3] - 0.9375).abs() < 1e-6);
}

/// A joint nobody measured is held, not driven from a guessed position.
///
/// `NaN` is what `skynet-obs` writes above a declared joint count and what a driver reports for a
/// quantity it does not measure. Commanding from an assumed zero would drive the joint to its
/// origin *because* nothing measured it, which is the worst possible response to missing data.
#[test]
fn an_unmeasured_joint_is_masked_out_rather_than_driven_from_zero() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut commands = Ring::open(&words).expect("the same ring").consumer();

    let region = obs_words();
    let obs_ring = skynet_obs::Ring::create(&region, 4).expect("an observation ring");
    let mut body = body_at(2, 0, &[0.2, 0.0]);
    body.position[1] = f32::NAN;
    obs_ring.producer().publish(&body).expect("published");
    let mut observations = skynet_obs::Ring::open(&region)
        .expect("the same ring")
        .consumer();

    let mut host = PolicyHost::new(
        Approach::toward(&[1.0, 1.0], 0.5),
        still(),
        lease(2),
        10_000_000,
    );
    host.turn(&mut producer, Some(&mut observations));

    let mut command = skynet_ring::Command::default();
    commands.read(&mut command);
    assert_eq!(command.mask[0], 1, "the measured joint is commanded");
    assert!((command.values[0] - 0.6).abs() < 1e-6);
    assert_eq!(command.mask[1], 0, "and the unmeasured one is held");
}

/// A closed-loop policy with no body commands nothing.
#[test]
fn a_closed_loop_backend_with_no_body_holds() {
    let words = ring_words();
    let ring = Ring::create(&words, SLOTS).expect("a ring");
    let mut producer = ring.producer();
    let mut host = PolicyHost::new(Approach::toward(&[1.0], 0.5), still(), lease(1), 10_000_000);

    assert_eq!(
        host.turn(&mut producer, None),
        Tick::Held(1),
        "alive, and commanding nothing, because there is nothing to close on",
    );
}

/// The complete §25 loop: body → policy → kernel → body, with nothing hand-written in it.
///
/// This is the diagram at the top of §25, running. A driver is read; the state crosses the
/// observation ring; a backend computes a command **from what it read**; the command crosses the
/// command ring; the eight guards evaluate it; the driver is written. Then it happens again, and
/// the second command is different because the first one moved the robot.
///
/// Every previous end-to-end test in this repository was missing the first arrow. The command was
/// either hand-written into the ring or produced by an open-loop table, so the loop was a line: no
/// value ever travelled from the body back to the thing deciding what to do to it. This closes it.
#[test]
fn the_body_reaches_the_policy_and_the_policy_moves_the_body() {
    let command_region = ring_words();
    let command_ring = Ring::create(&command_region, SLOTS).expect("a command ring");
    let mut commands_out = command_ring.producer();
    let mut commands_in = Ring::open(&command_region)
        .expect("the same ring")
        .consumer();

    let body_region = obs_words();
    let body_ring = skynet_obs::Ring::create(&body_region, 4).expect("an observation ring");
    let mut bodies_out = body_ring.producer();
    let mut bodies_in = skynet_obs::Ring::open(&body_region)
        .expect("the same ring")
        .consumer();

    let channels: [&str; 1] = ["shoulder"];
    let per_channel: [(&str, f64); 1] = [("shoulder", 10.0)];
    let mut active: FixedVec<Guard, 8> = FixedVec::new();
    active.push(Guard::Envelope);
    let limits = ResolvedLimits {
        tier: Tier::Host,
        enforcement: Enforcement::Enforcing,
        joint_velocity_max_rad_s: Some(20.0),
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        min_control_hz: Some(100.0),
        max_delta_per_cycle: None,
        proximity: None,
        per_channel: &per_channel,
        active_guards: active,
        unavailable_guards: FixedVec::new(),
        source: Some("the closed-loop test"),
        notices: FixedVec::new(),
        grants: &[],
        frames: None,
        workspace: &[],
        self_collision_margin_m: None,
    };

    // Halve the distance to 1.0 each cycle, reading where the joint actually is.
    let mut policy = PolicyHost::new(Approach::toward(&[1.0], 0.5), still(), lease(1), 10_000_000);
    let mut kernel = Host::new(&channels);
    let mut robot = Fake::new(1);

    let mut commanded = Vec::new();
    for cycle in 0..4u64 {
        // The kernel host reads the body and publishes it. On the first cycle the policy has
        // posted nothing, so the driver is written a hold — which is the state §33.1 describes.
        kernel.step_observing(
            cycle * 10_000_000,
            &mut robot,
            &mut commands_in,
            &limits,
            None::<&Envelope<'_>>,
            Some(&mut bodies_out),
        );

        // The policy reads that body and decides.
        let tick = policy.turn(&mut commands_out, Some(&mut bodies_in));
        assert_eq!(tick, Tick::Published(cycle + 1), "cycle {cycle}");

        // And the next kernel turn applies it.
        let turn = kernel.step_observing(
            cycle * 10_000_000 + 5_000_000,
            &mut robot,
            &mut commands_in,
            &limits,
            None::<&Envelope<'_>>,
            Some(&mut bodies_out),
        );
        assert_eq!(u64::from(turn.cycle.command_seq), cycle + 1);
        commanded.push(robot.position[0]);
    }

    // 0 → 0.5 → 0.75 → 0.875 → 0.9375. Each value is a function of the previous *measured*
    // position, which only this arrangement can produce: the numbers come out of the driver, not
    // out of the test.
    assert!((commanded[0] - 0.5).abs() < 1e-6, "half of the way from 0");
    assert!(
        (commanded[1] - 0.75).abs() < 1e-6,
        "and half of what remained, which it could only know by reading the body",
    );
    assert!((commanded[2] - 0.875).abs() < 1e-6);
    assert!((commanded[3] - 0.9375).abs() < 1e-6);

    assert_eq!(
        policy.summary().unbudgeted,
        4,
        "and every one of those cycles is counted as a reading nothing could vouch for, because \
         this lease declared no staleness budget",
    );
}

/// The body a policy reads is the one the driver reported, not the one it was commanded.
///
/// The distinction that makes an observation ring worth having. A guard clamped the command, so
/// what the robot did and what the policy asked for are different — and the next observation
/// carries the first, which is the only thing that lets a policy notice it is being clamped.
#[test]
fn a_clamped_command_comes_back_as_the_body_the_robot_actually_reached() {
    let command_region = ring_words();
    let command_ring = Ring::create(&command_region, SLOTS).expect("a command ring");
    let mut commands_out = command_ring.producer();
    let mut commands_in = Ring::open(&command_region)
        .expect("the same ring")
        .consumer();

    let body_region = obs_words();
    let body_ring = skynet_obs::Ring::create(&body_region, 4).expect("an observation ring");
    let mut bodies_out = body_ring.producer();
    let mut bodies_in = skynet_obs::Ring::open(&body_region)
        .expect("the same ring")
        .consumer();

    let channels: [&str; 1] = ["shoulder"];
    // A ceiling of 0.25, far below where the policy is aiming.
    let per_channel: [(&str, f64); 1] = [("shoulder", 0.25)];
    let mut active: FixedVec<Guard, 8> = FixedVec::new();
    active.push(Guard::Envelope);
    let limits = ResolvedLimits {
        tier: Tier::Host,
        enforcement: Enforcement::Enforcing,
        joint_velocity_max_rad_s: Some(20.0),
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        min_control_hz: Some(100.0),
        max_delta_per_cycle: None,
        proximity: None,
        per_channel: &per_channel,
        active_guards: active,
        unavailable_guards: FixedVec::new(),
        source: Some("the clamped-feedback test"),
        notices: FixedVec::new(),
        grants: &[],
        frames: None,
        workspace: &[],
        self_collision_margin_m: None,
    };

    let mut policy = PolicyHost::new(Approach::toward(&[1.0], 1.0), still(), lease(1), 10_000_000);
    let mut kernel = Host::new(&channels);
    let mut robot = Fake::new(1);

    kernel.step_observing(
        0,
        &mut robot,
        &mut commands_in,
        &limits,
        None::<&Envelope<'_>>,
        Some(&mut bodies_out),
    );
    policy.turn(&mut commands_out, Some(&mut bodies_in));
    let turn = kernel.step_observing(
        5_000_000,
        &mut robot,
        &mut commands_in,
        &limits,
        None::<&Envelope<'_>>,
        Some(&mut bodies_out),
    );

    assert!(
        turn.cycle.tripped.contains(&Guard::Envelope),
        "the policy asked for 1.0 against a ceiling of 0.25",
    );
    assert!(
        (robot.position[0] - 0.25).abs() < 1e-6,
        "and got the ceiling"
    );

    // The next observation carries 0.25 — where the robot is — rather than 1.0, which is what it
    // was told to do. A policy reading the second would never learn it is being clamped.
    let mut seen = skynet_obs::Observation::default();
    kernel.step_observing(
        10_000_000,
        &mut robot,
        &mut commands_in,
        &limits,
        None::<&Envelope<'_>>,
        Some(&mut bodies_out),
    );
    assert_eq!(bodies_in.read(&mut seen), skynet_obs::Reading::Fresh);
    assert!(
        (seen.position[0] - 0.25).abs() < 1e-6,
        "the body is what the robot reached, not what the policy asked for",
    );
}
