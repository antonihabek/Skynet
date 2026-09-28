//! The Safety Kernel's own suite.
//!
//! These are the properties §7 states in prose, written down as assertions. Where a test's name
//! reads like a sentence from the specification, that is deliberate: a suite whose failures do not
//! tell you which paragraph broke is a suite people delete.

use super::*;

fn envelope() -> Envelope<'static> {
    Envelope {
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: Some(80.0),
        base_velocity_max_m_s: Some(1.5),
        watchdog_ms: Some(100.0),
        estop: Some(true),
        source: Some("https://example.invalid/datasheet.pdf"),
        basis: None,
    }
}

const PER_CHANNEL: [(&str, f64); 2] = [("elbow", 80.0), ("shoulder", 60.0)];

/// §86.3. Both joints are commanded in torque, which is what makes `joint_torque_max_nm` above
/// reach them at all: before this section neither whole-body figure was applied to any channel.
const JOINT_MODES: [(&str, &str); 2] = [("elbow", "torque"), ("shoulder", "torque")];

fn clamp() -> TransportClamp<'static> {
    TransportClamp {
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: Some(80.0),
        base_velocity_max_m_s: Some(1.5),
        min_control_hz: Some(10.0),
        per_channel: &PER_CHANNEL,
        joint_modes: &JOINT_MODES,
        travel: &[],
        source: Some("https://example.invalid/datasheet.pdf"),
    }
}

fn profile() -> OperatorProfile<'static> {
    OperatorProfile {
        name: "cell-a",
        max_delta_per_cycle: Some(0.25),
        ..Default::default()
    }
}

fn available() -> Available {
    Available {
        imu: true,
        people_detection: true,
        legged: true,
        // The fixture robot has a producer, so `proximity` resolves active and every existing case
        // about it keeps testing what it was written to test. §80's third state is about a robot
        // that has the sensor and not the producer, and it gets cases of its own below.
        proximity_reading: true,
    }
}

fn host_limits() -> ResolvedLimits<'static> {
    resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    })
}

fn cycle<'a>(desired: &'a [(&'a str, f64)]) -> CommandCycle<'a> {
    CommandCycle {
        now_ms: 1_000.0,
        command_ms: 995.0,
        sequence: 42,
        desired,
        previous: None,
        capability: None,
        nearest_person_m: None,
    }
}

fn value_of<'a>(verdict: &CycleVerdict<'a>, channel: &str) -> Option<f64> {
    verdict
        .command
        .iter()
        .find(|(name, _)| *name == channel)
        .map(|(_, value)| *value)
}

// ── Tiers and enforcement ────────────────────────────────────────────────────

#[test]
fn enforcement_is_derived_from_the_tier_never_supplied() {
    assert_eq!(Tier::Guest.enforcement(), Enforcement::Advisory);
    assert_eq!(Tier::Host.enforcement(), Enforcement::Enforcing);
    assert_eq!(
        Tier::Certified.enforcement(),
        Enforcement::EnforcingRedundant
    );
}

#[test]
fn only_the_certified_tier_has_a_stop_path_off_the_application_processor() {
    assert!(!Tier::Guest.independent_stop_path());
    assert!(!Tier::Host.independent_stop_path());
    assert!(Tier::Certified.independent_stop_path());
}

#[test]
fn an_unknown_tier_does_not_default_to_one() {
    assert_eq!(Tier::parse("host"), Some(Tier::Host));
    assert_eq!(Tier::parse("Host"), None);
    assert_eq!(Tier::parse("enforcing"), None);
}

// ── Limit composition ────────────────────────────────────────────────────────

#[test]
fn limits_compose_by_minimum_never_by_precedence() {
    let narrow_profile = OperatorProfile {
        name: "half-speed",
        joint_velocity_max_rad_s: Some(0.5),
        ..profile()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(narrow_profile),
        frames: None,
        available: available(),
    });
    assert_eq!(limits.joint_velocity_max_rad_s, Some(0.5));
}

#[test]
fn a_profile_cannot_widen_and_is_told_that_it_tried() {
    let greedy = OperatorProfile {
        name: "faster-please",
        joint_torque_max_nm: Some(500.0),
        ..profile()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(greedy),
        frames: None,
        available: available(),
    });
    assert_eq!(
        limits.joint_torque_max_nm,
        Some(80.0),
        "the minimum discards the widening"
    );
    let widened = limits
        .notices
        .iter()
        .find(|n| n.code == NoticeCode::ProfileWidened)
        .expect("the attempt is recorded rather than silently ignored");
    assert!(widened.subjects.contains(&"joint_torque_max_nm"));
}

#[test]
fn an_absent_source_never_widens_the_result() {
    // Only the transport clamp declares a torque ceiling; the answer is that ceiling, not None.
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: None,
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    assert_eq!(limits.joint_torque_max_nm, Some(80.0));
}

#[test]
fn a_body_with_no_envelope_says_so_rather_than_assuming_one() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: None,
        transport: None,
        profile: None,
        frames: None,
        available: available(),
    });
    assert!(limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::NoEnvelope));
    assert_eq!(limits.joint_torque_max_nm, None);
    assert_eq!(limits.source, None);
}

#[test]
fn limits_without_provenance_are_reported_as_such() {
    let unsourced = Envelope {
        source: None,
        ..envelope()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(unsourced),
        transport: None,
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    assert!(limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::UnsourcedLimits));
}

#[test]
fn the_guest_tier_says_it_enforces_nothing() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Guest,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    assert_eq!(limits.enforcement, Enforcement::Advisory);
    assert!(limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::AdvisoryOnly));
}

// ── Guard availability ───────────────────────────────────────────────────────

#[test]
fn a_guard_whose_inputs_are_missing_is_unavailable_not_passing() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    assert!(!limits.is_active(Guard::SelfCollision));
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::SelfCollision)
        .expect("reported, with the missing input named");
    assert!(entry.missing.contains(&"embodiment.frames"));
}

#[test]
fn balance_on_a_wheeled_base_reads_as_inapplicable_not_broken() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: Available {
            legged: false,
            imu: false,
            ..available()
        },
    });
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::Balance)
        .unwrap();
    assert!(
        entry
            .missing
            .contains(&"a legged base — not applicable to this body"),
        "a shape difference must not read as a defect"
    );
    assert!(
        !entry.missing.contains(&"an IMU"),
        "the IMU is not the reason on a body that cannot fall over this way"
    );
}

#[test]
fn a_legged_body_without_an_imu_is_missing_an_imu() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: Available {
            imu: false,
            ..available()
        },
    });
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::Balance)
        .unwrap();
    assert!(entry.missing.contains(&"an IMU"));
}

#[test]
fn no_jerk_ceiling_disables_the_rate_limiter_and_says_so_twice() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: None,
        frames: None,
        available: available(),
    });
    assert!(!limits.is_active(Guard::RateLimit));
    assert!(limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::NoRateLimit));
    assert!(limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::GuardUnavailable && n.guard == Some(Guard::RateLimit)));
}

#[test]
fn every_guard_is_either_active_or_reported_unavailable() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: None,
        transport: None,
        profile: None,
        frames: None,
        available: Available::default(),
    });
    assert_eq!(
        limits.active_guards.len() + limits.unavailable_guards.len(),
        GUARDS.len(),
        "no guard may be silently absent from both lists"
    );
}

// ── The cycle ────────────────────────────────────────────────────────────────

#[test]
fn a_command_inside_every_ceiling_passes_through_untouched() {
    let limits = host_limits();
    let desired = [("elbow", 40.0), ("shoulder", 10.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(verdict.response, None);
    assert!(verdict.tripped.is_empty());
    assert_eq!(value_of(&verdict, "elbow"), Some(40.0));
}

#[test]
fn a_command_above_a_joint_rating_is_clamped_to_it() {
    let limits = host_limits();
    let desired = [("elbow", 120.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(verdict.response, Some(FaultResponse::Clamp));
    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert_eq!(value_of(&verdict, "elbow"), Some(80.0));
    assert_eq!(
        verdict.clamped.get(0),
        Some(&Clamped {
            channel: "elbow",
            from: 120.0,
            to: 80.0
        })
    );
}

#[test]
fn clamping_preserves_the_sign() {
    let limits = host_limits();
    let desired = [("elbow", -120.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(value_of(&verdict, "elbow"), Some(-80.0));
}

#[test]
fn a_stale_command_is_discarded_never_clamped_and_forwarded() {
    let limits = host_limits();
    let desired = [("elbow", 500.0)];
    let mut stale = cycle(&desired);
    stale.command_ms = 800.0; // 200 ms old against a 100 ms window
    let verdict = evaluate_cycle(&stale, &limits, Some(&envelope()), None);
    assert_eq!(verdict.response, Some(FaultResponse::StopCat2));
    assert_eq!(verdict.tripped.iter().count(), 1);
    assert!(verdict.tripped.contains(&Guard::Watchdog));
    assert!(
        verdict.command.is_empty(),
        "the dead producer's intention must not survive at a legal magnitude"
    );
}

#[test]
fn a_repeated_sequence_number_is_a_stalled_producer() {
    let limits = host_limits();
    let desired = [("elbow", 10.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), Some(42));
    assert!(verdict.tripped.contains(&Guard::Watchdog));
    assert_eq!(verdict.response, Some(FaultResponse::StopCat2));
}

#[test]
fn an_advancing_sequence_is_not_a_stall() {
    let limits = host_limits();
    let desired = [("elbow", 10.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), Some(41));
    assert!(!verdict.tripped.contains(&Guard::Watchdog));
}

#[test]
fn the_watchdog_cannot_fire_when_no_window_was_declared() {
    let no_window = Envelope {
        watchdog_ms: None,
        ..envelope()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(no_window),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 10.0)];
    let mut ancient = cycle(&desired);
    ancient.command_ms = 0.0;
    let verdict = evaluate_cycle(&ancient, &limits, Some(&no_window), Some(999));
    assert!(
        !verdict.tripped.contains(&Guard::Watchdog),
        "a guard with no input does not fire; it is reported unavailable"
    );
}

#[test]
fn rate_limiting_runs_after_magnitude_clamping() {
    // The clamp itself creates the step change the rate limiter then has to turn into a ramp. If
    // the order were reversed, this command would leave the kernel at 80 in one cycle.
    let limits = host_limits();
    let desired = [("elbow", 400.0)];
    let previous = [("elbow", 0.0)];
    let mut c = cycle(&desired);
    c.previous = Some(&previous);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);

    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert!(verdict.tripped.contains(&Guard::RateLimit));
    // 80 Nm ceiling × 0.25 per cycle = a 20 Nm step.
    assert_eq!(value_of(&verdict, "elbow"), Some(20.0));
    assert_eq!(verdict.clamped.len(), 2, "both stages are recorded");
    assert_eq!(verdict.clamped.get(0).unwrap().to, 80.0);
    assert_eq!(verdict.clamped.get(1).unwrap().to, 20.0);
}

#[test]
fn a_channel_with_no_ceiling_rate_limits_against_its_own_previous_value() {
    let limits = host_limits();
    let desired = [("wrist", 100.0)];
    let previous = [("wrist", 10.0)];
    let mut c = cycle(&desired);
    c.previous = Some(&previous);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    // No per-channel ceiling for "wrist", so the step is 10 × 0.25 = 2.5.
    assert_eq!(value_of(&verdict, "wrist"), Some(12.5));
}

#[test]
fn the_base_channel_is_bounded_by_the_base_velocity_ceiling() {
    let limits = host_limits();
    let desired = [("base", 9.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(value_of(&verdict, "base"), Some(1.5));
}

#[test]
fn advisory_mode_forwards_the_desired_command_and_still_records_the_verdict() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Guest,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 120.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);

    assert!(verdict.advisory);
    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert_eq!(
        verdict.clamped.get(0).unwrap().to,
        80.0,
        "the verdict is computed"
    );
    assert_eq!(
        value_of(&verdict, "elbow"),
        Some(120.0),
        "and the command still reaches the robot unchanged"
    );
}

#[test]
fn an_advisory_stop_still_forwards_because_there_is_nothing_underneath_to_stop() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Guest,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 10.0)];
    let mut stale = cycle(&desired);
    stale.command_ms = 0.0;
    let verdict = evaluate_cycle(&stale, &limits, Some(&envelope()), None);
    assert_eq!(verdict.response, Some(FaultResponse::StopCat2));
    assert!(verdict.advisory);
    assert_eq!(verdict.command.len(), 1);
}

// ── Guards that need inputs the reference vectors do not carry ───────────────

#[test]
fn the_mode_gate_holds_a_command_whose_capability_was_not_granted() {
    let grants: [&str; 1] = ["sensing.rgb"];
    let limits = host_limits().with_grants(&grants);
    let desired = [("elbow", 10.0)];
    let mut c = cycle(&desired);
    c.capability = Some("manipulation.grasp");
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    assert!(verdict.tripped.contains(&Guard::ModeGate));
    assert_eq!(verdict.response, Some(FaultResponse::Hold));
    assert!(verdict.command.is_empty());
}

#[test]
fn the_mode_gate_passes_a_granted_capability() {
    let grants: [&str; 2] = ["manipulation.grasp", "sensing.rgb"];
    let limits = host_limits().with_grants(&grants);
    let desired = [("elbow", 10.0)];
    let mut c = cycle(&desired);
    c.capability = Some("manipulation.grasp");
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    assert!(!verdict.tripped.contains(&Guard::ModeGate));
}

#[test]
fn a_cycle_that_names_no_capability_does_not_trip_the_gate() {
    // The caller did not say; that is not the same as "no capability", and a guard with no input
    // never passes or fails by default.
    let limits = host_limits();
    let desired = [("elbow", 10.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert!(!verdict.tripped.contains(&Guard::ModeGate));
}

#[test]
fn proximity_stops_inside_the_stop_distance() {
    let prox = Proximity {
        stop_distance_m: 0.3,
        slow_distance_m: 1.0,
        slow_factor: 0.3,
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 10.0)];
    let mut c = cycle(&desired);
    c.nearest_person_m = Some(0.2);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    assert!(verdict.tripped.contains(&Guard::Proximity));
    assert_eq!(verdict.response, Some(FaultResponse::StopCat1));
    assert!(verdict.command.is_empty());
}

#[test]
fn proximity_slows_between_the_two_distances() {
    let prox = Proximity {
        stop_distance_m: 0.3,
        slow_distance_m: 1.0,
        slow_factor: 0.5,
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 40.0)];
    let mut c = cycle(&desired);
    c.nearest_person_m = Some(0.8);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    assert!(verdict.tripped.contains(&Guard::Proximity));
    assert_eq!(value_of(&verdict, "elbow"), Some(20.0));
}

#[test]
fn proximity_does_nothing_when_nothing_is_measuring_separation() {
    let limits = host_limits();
    let desired = [("elbow", 40.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert!(!verdict.tripped.contains(&Guard::Proximity));
    assert_eq!(value_of(&verdict, "elbow"), Some(40.0));
}

// ── Composition of responses ─────────────────────────────────────────────────

#[test]
fn the_response_is_the_strongest_any_tripped_guard_demanded() {
    let prox = Proximity {
        stop_distance_m: 0.3,
        slow_distance_m: 1.0,
        slow_factor: 0.5,
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 500.0)];

    // Inside the stop distance: the envelope demands `clamp`, proximity demands `stop_cat1`, and
    // the stronger wins.
    let mut c = cycle(&desired);
    c.nearest_person_m = Some(0.2);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert!(verdict.tripped.contains(&Guard::Proximity));
    assert_eq!(verdict.response, Some(FaultResponse::StopCat1));
    assert!(verdict.command.is_empty(), "a stop sends nothing");
}

#[test]
fn slowing_for_a_person_is_reported_as_a_reduction_and_not_as_a_stop() {
    // §7.1's row for `proximity` is `stop_cat1`, and that is what it does inside the stop distance.
    // Between the two distances it scales the command and lets it through, and a verdict deriving
    // its response from the guard's row said "stop_cat1" about a cycle the robot kept moving
    // through. A black box recording that would report a stop that did not happen.
    let prox = Proximity {
        stop_distance_m: 0.3,
        slow_distance_m: 1.0,
        slow_factor: 0.5,
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
        frames: None,
        available: available(),
    });
    let desired = [("elbow", 40.0)];
    let mut c = cycle(&desired);
    c.nearest_person_m = Some(0.8);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    assert!(verdict.tripped.contains(&Guard::Proximity));
    assert_eq!(verdict.response, Some(FaultResponse::Clamp));
    assert_eq!(value_of(&verdict, "elbow"), Some(20.0));
}

#[test]
fn responses_are_ordered_weakest_to_strongest() {
    assert!(FaultResponse::Clamp < FaultResponse::Hold);
    assert!(FaultResponse::Hold < FaultResponse::StopCat2);
    assert!(FaultResponse::StopCat2 < FaultResponse::StopCat1);
    assert!(FaultResponse::StopCat1 < FaultResponse::StopCat0);
}

// ── Overflow ─────────────────────────────────────────────────────────────────

#[test]
fn more_channels_than_the_kernel_can_evaluate_is_a_fault_not_a_truncation() {
    let limits = host_limits();
    let names: [&str; MAX_CHANNELS + 4] = ["j"; MAX_CHANNELS + 4];
    let desired: [(&str, f64); MAX_CHANNELS + 4] = core::array::from_fn(|i| (names[i], i as f64));
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(verdict.response, Some(FaultResponse::StopCat2));
    assert!(verdict.command.is_empty());
}

// ── can_start ────────────────────────────────────────────────────────────────

#[test]
fn a_refused_transport_plan_cannot_start() {
    assert!(can_start(true, &host_limits(), WarningChannel::Ready).is_err());
}

#[test]
fn a_tier_that_promised_enforcement_and_resolved_to_advisory_cannot_start() {
    // Not reachable through `resolve_limits`, which derives enforcement — so this is a check on
    // a limit set assembled some other way, which is exactly when it is worth having.
    let mut limits = host_limits();
    limits.enforcement = Enforcement::Advisory;
    assert!(can_start(false, &limits, WarningChannel::Ready).is_err());
}

#[test]
fn a_guest_tier_robot_starts_despite_advisory_enforcement() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Guest,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    assert!(can_start(false, &limits, WarningChannel::Ready).is_ok());
}

#[test]
fn notices_and_imperfections_do_not_block_a_start() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: None,
        transport: None,
        profile: None,
        frames: None,
        available: Available::default(),
    });
    assert!(
        can_start(false, &limits, WarningChannel::Ready).is_ok(),
        "a system that refuses to start on every imperfection is a system that gets bypassed"
    );
}

// ── §81.3: the body that cannot warn ──────────────────────────────────────────

/// A certified limit set. `resolve_limits` will not hand one out for a body this sparse, which is
/// the point: this is the tier claiming its guarantees, assembled directly so the interlock can be
/// asked about it.
fn certified_limits() -> ResolvedLimits<'static> {
    let mut limits = host_limits();
    limits.tier = Tier::Certified;
    limits.enforcement = Enforcement::EnforcingRedundant;
    limits
}

#[test]
fn a_certified_robot_with_no_declared_warning_cannot_start() {
    let error = can_start(false, &certified_limits(), WarningChannel::Undeclared)
        .expect_err("the certified tier's meaning is that the declared controls are present");
    assert!(error.contains("no warning channel is declared"));
}

#[test]
fn a_certified_robot_whose_warning_failed_cannot_start() {
    let error = can_start(false, &certified_limits(), WarningChannel::Failed)
        .expect_err("a declared channel that did not sound is a broken control, not a missing one");
    // Reported apart from `Undeclared` because the fix is different: a provisioning script that
    // never finished, versus a speaker that is broken.
    assert!(error.contains("did not sound"));
}

#[test]
fn a_certified_robot_that_can_warn_starts() {
    assert!(can_start(false, &certified_limits(), WarningChannel::Ready).is_ok());
}

#[test]
fn a_host_robot_without_a_warning_channel_still_starts() {
    // §81.3 scopes the refusal to the certified tier deliberately. At host the incident and the
    // console line stand, and refusing every bench robot without a speaker is the failure
    // `notices_and_imperfections_do_not_block_a_start` is about.
    assert!(can_start(false, &host_limits(), WarningChannel::Undeclared).is_ok());
    assert!(can_start(false, &host_limits(), WarningChannel::Failed).is_ok());
}

#[test]
fn a_guest_robot_without_a_warning_channel_still_starts() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Guest,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    assert!(can_start(false, &limits, WarningChannel::Undeclared).is_ok());
}

#[test]
fn the_warning_channel_spellings_are_the_wire_ones() {
    assert_eq!(WarningChannel::Ready.as_str(), "ready");
    assert_eq!(WarningChannel::Failed.as_str(), "failed");
    assert_eq!(WarningChannel::Undeclared.as_str(), "undeclared");
}

// ── Determinism ──────────────────────────────────────────────────────────────

#[test]
fn the_same_cycle_evaluated_twice_gives_the_same_verdict() {
    let limits = host_limits();
    let desired = [("elbow", 120.0), ("shoulder", 5.0)];
    let previous = [("elbow", 0.0), ("shoulder", 5.0)];
    let mut c = cycle(&desired);
    c.previous = Some(&previous);

    let first = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    let second = evaluate_cycle(&c, &limits, Some(&envelope()), None);

    assert_eq!(first.response, second.response);
    assert_eq!(first.tripped.len(), second.tripped.len());
    assert_eq!(first.clamped.len(), second.clamped.len());
    for i in 0..first.command.len() {
        assert_eq!(first.command.get(i), second.command.get(i));
    }
}

#[test]
fn signum_does_not_turn_zero_into_full_scale() {
    // core::f64::signum(0.0) is 1.0, which under a zero ceiling would command the ceiling.
    assert_eq!(signum(0.0), 0.0);
    assert_eq!(signum(-0.0), 0.0);
    assert_eq!(signum(-3.0), -1.0);
}

// ── The pose guards (§7.1 rows four and five, §44) ───────────────────────────

use crate::kinematics::{
    Capsule, Frame, FrameSet, Joint, JointBinding, JointKind, Kinematics, Shape, Transform, Volume,
    WorkspaceLayer,
};

/// A one-metre arm on a hinge about z, with a capsule down its length.
fn arm_frames() -> [Frame<'static>; 3] {
    [
        Frame {
            name: "base_link",
            parent: None,
            local: Transform::IDENTITY,
            joint: None,
            collision: None,
            ignore: &[],
        },
        Frame {
            name: "upper",
            parent: Some(0),
            local: Transform::IDENTITY,
            joint: Some(Joint {
                name: "shoulder",
                kind: JointKind::Revolute,
                axis: [0.0, 0.0, 1.0],
                mimic: None,
            }),
            collision: Some(Capsule {
                a: [0.0, 0.0, 0.0],
                b: [1.0, 0.0, 0.0],
                radius_m: 0.05,
            }),
            ignore: &[],
        },
        Frame {
            name: "tool",
            parent: Some(1),
            local: Transform {
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            joint: None,
            collision: None,
            ignore: &[],
        },
    ]
}

const ARM_BINDINGS: [JointBinding<'static>; 1] = [JointBinding {
    joint: "shoulder",
    channel: Some("shoulder"),
    measured: 0.0,
}];

fn pose_limits<'a>(
    frames: &'a [Frame<'a>],
    workspace: &'a [WorkspaceLayer<'a>],
    margin: Option<f64>,
) -> ResolvedLimits<'a> {
    let set = FrameSet::new(frames).expect("the fixture is topological");
    resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: None,
        profile: Some(OperatorProfile {
            name: "cell-a",
            workspace,
            self_collision_margin_m: margin,
            ..Default::default()
        }),
        frames: Some(set),
        available: available(),
    })
}

#[test]
fn the_workspace_guard_needs_volumes_and_a_tree_and_names_which_is_missing() {
    let frames = arm_frames();
    let none = pose_limits(&frames, &[], None);
    let entry = none
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::Workspace)
        .expect("unavailable, not passing");
    assert!(entry.missing.contains(&"profile.workspace"));

    // And the other way round: volumes, no tree.
    let volumes = [Volume {
        frame: "tool",
        shape: Shape::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius_m: 2.0,
        },
    }];
    let layers = [WorkspaceLayer {
        name: "cell",
        volumes: &volumes,
    }];
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: None,
        profile: Some(OperatorProfile {
            name: "cell-a",
            workspace: &layers,
            ..Default::default()
        }),
        frames: None,
        available: available(),
    });
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::Workspace)
        .expect("unavailable");
    assert!(entry.missing.contains(&"embodiment.frames"));
}

#[test]
fn a_volume_naming_a_frame_the_body_does_not_have_names_that_frame() {
    // §44.5: "the tree does not reach the end effector" is reported with the frame named, not as a
    // generic malformation an operator cannot act on.
    let frames = arm_frames();
    let volumes = [Volume {
        frame: "left_hand.tool0",
        shape: Shape::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius_m: 2.0,
        },
    }];
    let layers = [WorkspaceLayer {
        name: "cell",
        volumes: &volumes,
    }];
    let limits = pose_limits(&frames, &layers, None);
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::Workspace)
        .expect("unavailable");
    assert!(entry.missing.contains(&"left_hand.tool0"));
}

#[test]
fn a_commanded_pose_inside_the_allowed_volume_passes() {
    let frames = arm_frames();
    let volumes = [Volume {
        frame: "tool",
        shape: Shape::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius_m: 2.0,
        },
    }];
    let layers = [WorkspaceLayer {
        name: "cell",
        volumes: &volumes,
    }];
    let limits = pose_limits(&frames, &layers, None);
    assert!(limits.is_active(Guard::Workspace));

    let desired = [("shoulder", 0.0)];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(&desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &ARM_BINDINGS,
            scratch: &mut scratch,
        }),
    );
    assert!(verdict.pose.evaluated);
    assert!(verdict.pose.lookahead);
    assert!(verdict.pose.trips.is_empty());
    assert!(!verdict.tripped.contains(&Guard::Workspace));
}

#[test]
fn a_commanded_pose_outside_every_allowed_volume_holds_the_command() {
    let frames = arm_frames();
    // A half-metre sphere at the origin. The tool is a metre out, whatever the shoulder does.
    let volumes = [Volume {
        frame: "tool",
        shape: Shape::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius_m: 0.5,
        },
    }];
    let layers = [WorkspaceLayer {
        name: "cell",
        volumes: &volumes,
    }];
    let limits = pose_limits(&frames, &layers, None);

    let desired = [("shoulder", 0.0)];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(&desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &ARM_BINDINGS,
            scratch: &mut scratch,
        }),
    );
    assert!(verdict.tripped.contains(&Guard::Workspace));
    assert_eq!(verdict.response, Some(FaultResponse::Hold));
    assert!(verdict.command.is_empty(), "a hold sends nothing");

    let trip = verdict.pose.trips.get(0).expect("one trip");
    assert_eq!(trip.frame, "tool");
    assert_eq!(trip.other, None);
    // A metre out of a half-metre sphere is half a metre outside it.
    let margin = trip.margin_m.expect("the pose resolved");
    assert!((margin + 0.5).abs() < 1e-12, "margin was {margin}");
}

#[test]
fn layers_compose_by_conjunction_so_a_later_layer_can_only_narrow() {
    let frames = arm_frames();
    let wide = [Volume {
        frame: "tool",
        shape: Shape::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius_m: 5.0,
        },
    }];
    let narrow = [Volume {
        frame: "tool",
        shape: Shape::Sphere {
            centre: [0.0, 0.0, 0.0],
            radius_m: 0.5,
        },
    }];
    let layers = [
        WorkspaceLayer {
            name: "site",
            volumes: &wide,
        },
        WorkspaceLayer {
            name: "cell",
            volumes: &narrow,
        },
    ];
    let limits = pose_limits(&frames, &layers, None);
    let desired = [("shoulder", 0.0)];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(&desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &ARM_BINDINGS,
            scratch: &mut scratch,
        }),
    );
    assert!(
        verdict.tripped.contains(&Guard::Workspace),
        "the wide layer cannot re-permit what the narrow one forbade"
    );
}

#[test]
fn one_layer_with_two_volumes_admits_either_of_them() {
    // Within a layer the volumes are alternatives — an operator who draws two allowed regions
    // means either, and a rule that required both would forbid everything.
    let frames = arm_frames();
    let volumes = [
        Volume {
            frame: "tool",
            shape: Shape::Sphere {
                centre: [5.0, 0.0, 0.0],
                radius_m: 0.5,
            },
        },
        Volume {
            frame: "tool",
            shape: Shape::Sphere {
                centre: [1.0, 0.0, 0.0],
                radius_m: 0.5,
            },
        },
    ];
    let layers = [WorkspaceLayer {
        name: "cell",
        volumes: &volumes,
    }];
    let limits = pose_limits(&frames, &layers, None);
    let desired = [("shoulder", 0.0)];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(&desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &ARM_BINDINGS,
            scratch: &mut scratch,
        }),
    );
    assert!(verdict.pose.trips.is_empty());
}

#[test]
fn the_pose_checked_is_the_one_after_clamping_and_not_the_one_asked_for() {
    // The whole reason these guards sit at rows four and five rather than at row one. A policy
    // asking for a wild angle that the envelope clamps must be judged on where the clamped command
    // puts the arm — anything else reports an excursion the kernel already prevented.
    let frames = arm_frames();
    let volumes = [Volume {
        frame: "tool",
        shape: Shape::Box {
            // A slab that contains the arm pointing along +x and nothing else.
            min: [0.5, -0.2, -0.2],
            max: [1.5, 0.2, 0.2],
        },
    }];
    let layers = [WorkspaceLayer {
        name: "cell",
        volumes: &volumes,
    }];
    let set = FrameSet::new(&frames).unwrap();
    let per_channel = [("shoulder", 0.0)];
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(TransportClamp {
            per_channel: &per_channel,
            ..Default::default()
        }),
        profile: Some(OperatorProfile {
            name: "cell-a",
            workspace: &layers,
            ..Default::default()
        }),
        frames: Some(set),
        available: available(),
    });

    // The policy asks for a quarter turn, which would put the tool at (0, 1, 0) — outside the
    // slab. The channel ceiling is zero, so the command that goes out is zero and the tool stays
    // at (1, 0, 0), which is inside it.
    let desired = [("shoulder", core::f64::consts::FRAC_PI_2)];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(&desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &ARM_BINDINGS,
            scratch: &mut scratch,
        }),
    );
    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert!(
        !verdict.tripped.contains(&Guard::Workspace),
        "judged on the clamped command, not the desired one"
    );
}

// ── self_collision ───────────────────────────────────────────────────────────

/// Two capsules on a shared root, a metre apart, each half a metre long.
fn pair_frames() -> [Frame<'static>; 3] {
    let capsule = Capsule {
        a: [0.0, 0.0, 0.0],
        b: [0.5, 0.0, 0.0],
        radius_m: 0.1,
    };
    [
        Frame {
            name: "base_link",
            parent: None,
            local: Transform::IDENTITY,
            joint: None,
            collision: None,
            ignore: &[],
        },
        Frame {
            name: "left",
            parent: Some(0),
            local: Transform {
                translation: [0.0, 0.5, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            joint: None,
            collision: Some(capsule),
            ignore: &[],
        },
        Frame {
            name: "right",
            parent: Some(0),
            local: Transform {
                translation: [0.0, -0.5, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            joint: None,
            collision: Some(capsule),
            ignore: &[],
        },
    ]
}

/// Run one cycle against a collision fixture and hand the verdict to the assertions.
///
/// A closure rather than a return value: the verdict borrows the frame names from the caller's
/// array, which is exactly the arrangement the runtime has — the frame set lives as long as the
/// lease and the verdict is read before the next cycle overwrites it.
fn run_collision(frames: &[Frame<'_>], margin: Option<f64>, check: impl FnOnce(&CycleVerdict<'_>)) {
    let limits = pose_limits(frames, &[], margin);
    let desired: &[(&str, f64)] = &[];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &[],
            scratch: &mut scratch,
        }),
    );
    check(&verdict);
}

#[test]
fn two_bodies_that_are_far_apart_do_not_trip() {
    let frames = pair_frames();
    run_collision(&frames, Some(0.05), |verdict| {
        assert!(verdict.pose.trips.is_empty());
        assert!(!verdict.tripped.contains(&Guard::SelfCollision));
    });
}

#[test]
fn two_bodies_inside_the_margin_trip_and_the_verdict_names_both() {
    let mut frames = pair_frames();
    // Bring them to 0.2 m apart centre to centre: 0.2 − 0.1 − 0.1 = 0 clearance, and any margin
    // at all is then a violation.
    frames[1].local.translation = [0.0, 0.1, 0.0];
    frames[2].local.translation = [0.0, -0.1, 0.0];
    run_collision(&frames, Some(0.05), |verdict| {
        assert!(verdict.tripped.contains(&Guard::SelfCollision));
        assert_eq!(verdict.response, Some(FaultResponse::Hold));
        let trip = verdict.pose.trips.get(0).expect("one trip");
        assert_eq!(trip.frame, "left");
        assert_eq!(trip.other, Some("right"));
        let margin = trip.margin_m.expect("both poses resolved");
        assert!((margin + 0.05).abs() < 1e-12, "margin was {margin}");
    });
}

#[test]
fn a_declared_exception_stops_a_pair_tripping_however_it_was_written() {
    let mut frames = pair_frames();
    frames[1].local.translation = [0.0, 0.1, 0.0];
    frames[2].local.translation = [0.0, -0.1, 0.0];
    // Written on one side only. A body whose torso overlaps its pelvis by design says so once.
    frames[1].ignore = &[2];
    run_collision(&frames, Some(0.05), |verdict| {
        assert!(verdict.pose.trips.is_empty());
    });
}

#[test]
fn a_parent_and_its_child_are_never_tested_against_each_other() {
    // They touch at the joint by construction, and no descriptor should have to declare it.
    let capsule = Capsule {
        a: [0.0, 0.0, 0.0],
        b: [0.5, 0.0, 0.0],
        radius_m: 0.1,
    };
    let mut frames = pair_frames();
    frames[2].parent = Some(1);
    frames[2].local.translation = [0.0, 0.0, 0.0];
    frames[1].collision = Some(capsule);
    frames[2].collision = Some(capsule);
    run_collision(&frames, Some(0.05), |verdict| {
        assert!(verdict.pose.trips.is_empty());
    });
}

#[test]
fn one_collision_body_is_not_a_guard() {
    // Nothing to collide with. Reported unavailable rather than active-and-silent, because a guard
    // that passes by having no pair to test is the "passes by not looking" failure.
    let mut frames = pair_frames();
    frames[2].collision = None;
    let set = FrameSet::new(&frames).unwrap();
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: None,
        profile: Some(profile()),
        frames: Some(set),
        available: available(),
    });
    assert!(!limits.is_active(Guard::SelfCollision));
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::SelfCollision)
        .expect("unavailable");
    assert!(entry
        .missing
        .contains(&"embodiment.frames[].collision, on two or more frames"));
}

#[test]
fn an_absent_collision_margin_is_a_notice_rather_than_silence() {
    let frames = pair_frames();
    let limits = pose_limits(&frames, &[], None);
    assert!(limits.is_active(Guard::SelfCollision));
    assert!(limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::NoCollisionMargin));
}

#[test]
fn a_configured_margin_produces_no_notice() {
    let frames = pair_frames();
    let limits = pose_limits(&frames, &[], Some(0.02));
    assert!(!limits
        .notices
        .iter()
        .any(|n| n.code == NoticeCode::NoCollisionMargin));
}

#[test]
fn a_guard_that_is_unavailable_does_not_run_and_the_pose_block_says_so() {
    // No frames at all: both pose guards are unavailable, forward kinematics never runs, and the
    // block reports `evaluated: false` rather than an empty trip list that looks like a pass.
    let limits = host_limits();
    let desired = [("elbow", 1.0)];
    let mut scratch = Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle(&desired),
        &limits,
        Some(&envelope()),
        None,
        Some(Body {
            joints: &[],
            scratch: &mut scratch,
        }),
    );
    assert!(!verdict.pose.evaluated);
    assert!(verdict.pose.trips.is_empty());
}

// ── §38.2's session override ─────────────────────────────────────────────────
//
// The fifth layer, and the only one a person sets at the machine. Two properties carry the whole
// design: it can **only narrow**, which is arithmetic rather than a rule to remember, and a robot
// under one **says so**, because §38.2's stated hazard is not a slow robot but a robot that is
// quietly slow for a week.

/// A session narrowing joint velocity to a quarter of the envelope.
fn session() -> SessionOverride<'static> {
    SessionOverride {
        name: "maintenance",
        joint_velocity_max_rad_s: Some(0.5),
        ..Default::default()
    }
}

#[test]
fn a_session_override_narrows_the_ceiling_in_force() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(session()),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });

    assert_eq!(limits.joint_velocity_max_rad_s, Some(0.5));
}

#[test]
fn a_session_override_cannot_widen_and_is_told_that_it_tried() {
    // §38.2: "Attempted widenings are recorded and discarded, so an operator who believes they
    // have raised a limit is told they have not." Both halves are asserted, and the second is the
    // one that would rot silently — a discarded widening with no notice is a bench session where
    // somebody thinks the robot is faster than it is.
    let greedy = SessionOverride {
        name: "faster-please",
        joint_velocity_max_rad_s: Some(9.0),
        joint_torque_max_nm: Some(500.0),
        ..Default::default()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(greedy),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });

    assert_eq!(limits.joint_velocity_max_rad_s, Some(2.0), "envelope holds");
    assert_eq!(limits.joint_torque_max_nm, Some(80.0), "envelope holds");

    let widened = limits
        .notices
        .iter()
        .find(|notice| notice.code == NoticeCode::SessionWidened)
        .expect("a session that over-reached is told so");
    // No `collect` — this crate does not link `alloc`, which is the property §23.1 makes
    // compile-time rather than a review promise, and a test that needed a `Vec` would break it.
    assert!(widened
        .subjects
        .iter()
        .any(|subject| *subject == "joint_velocity_max_rad_s"));
    assert!(widened
        .subjects
        .iter()
        .any(|subject| *subject == "joint_torque_max_nm"));
}

#[test]
fn a_session_is_measured_against_what_is_in_force_not_against_the_envelope() {
    // The distinction §38.2 draws with "each layer may only narrow what the layer above it
    // allows". A session asking for 1.0 under a profile allowing 0.5 has not widened the
    // *envelope* — 1.0 is well inside 2.0 — but it has asked to raise the number actually in
    // force, which is the one the operator can see and the one they will believe they changed.
    let narrow = OperatorProfile {
        name: "cell-a",
        joint_velocity_max_rad_s: Some(0.5),
        ..profile()
    };
    let over = SessionOverride {
        name: "bench",
        joint_velocity_max_rad_s: Some(1.0),
        ..Default::default()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(over),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(narrow),
        frames: None,
        available: available(),
    });

    assert_eq!(
        limits.joint_velocity_max_rad_s,
        Some(0.5),
        "the profile holds"
    );
    assert!(limits
        .notices
        .iter()
        .any(|notice| notice.code == NoticeCode::SessionWidened));
}

#[test]
fn a_widening_session_is_never_reported_as_a_widening_profile() {
    // Two documents, two sentences. Telling somebody their signed profile over-reached when it was
    // their bench override would send them to edit a file that is not the problem.
    let greedy = SessionOverride {
        name: "faster-please",
        joint_velocity_max_rad_s: Some(9.0),
        ..Default::default()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(greedy),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });

    assert!(!limits
        .notices
        .iter()
        .any(|notice| notice.code == NoticeCode::ProfileWidened));
}

#[test]
fn a_robot_under_a_session_override_says_so_and_names_it() {
    // §38.2's hazard is "a 'maintenance mode, half speed' that outlives the maintenance", and it is
    // a hazard because a slowed robot looks like a working one. The notice is how it stops looking
    // like one, and it carries the name so the sentence says what to clear.
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(session()),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });

    let notice = limits
        .notices
        .iter()
        .find(|notice| notice.code == NoticeCode::SessionOverride)
        .expect("an override announces itself");
    assert_eq!(notice.subjects.len(), 1);
    assert_eq!(notice.subjects.iter().next().copied(), Some("maintenance"));
}

#[test]
fn no_session_says_nothing_at_all() {
    // The other half of the sentence above, and the property that kept the corpus byte-identical:
    // a robot with no override emits neither notice, so every case written before this layer
    // existed resolves exactly as it did.
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });

    assert!(!limits.notices.iter().any(|notice| matches!(
        notice.code,
        NoticeCode::SessionOverride | NoticeCode::SessionWidened
    )));
}

#[test]
fn a_session_can_narrow_the_jerk_ceiling_below_the_profiles() {
    let gentler = SessionOverride {
        name: "gentle",
        max_delta_per_cycle: Some(0.05),
        ..Default::default()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(gentler),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });

    assert_eq!(limits.max_delta_per_cycle, Some(0.05));
}

#[test]
fn a_session_jerk_ceiling_turns_the_rate_limit_guard_on() {
    // The one field that can make a guard *runnable*. `rate_limit` is unavailable when nothing sets
    // `max_delta_per_cycle` (§7.2), so a bench override that sets one on a robot whose profile does
    // not has strictly reduced what the robot may do — which is what "only ever narrows" means,
    // even though it is the opposite of removing a guard.
    let bare = OperatorProfile {
        name: "cell-a",
        max_delta_per_cycle: None,
        ..Default::default()
    };
    let gentle = SessionOverride {
        name: "gentle",
        max_delta_per_cycle: Some(0.1),
        ..Default::default()
    };

    let without = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(bare),
        frames: None,
        available: available(),
    });
    assert!(!without.active_guards.iter().any(|g| *g == Guard::RateLimit));

    let with = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(gentle),
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(bare),
        frames: None,
        available: available(),
    });
    assert!(with.active_guards.iter().any(|g| *g == Guard::RateLimit));
    assert!(!with
        .notices
        .iter()
        .any(|notice| notice.code == NoticeCode::NoRateLimit));
}

#[test]
fn a_session_with_no_profile_at_all_still_narrows() {
    // An unprovisioned robot on a bench is the likeliest place for this layer to be used, and it is
    // the case where every layer above it is absent.
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: Some(session()),
        envelope: Some(envelope()),
        transport: None,
        profile: None,
        frames: None,
        available: available(),
    });

    assert_eq!(limits.joint_velocity_max_rad_s, Some(0.5));
}

/// The notice spellings `skynet-contracts::codes` hand-pins, pinned here too. The catalog
/// cannot enumerate what the kernel names (the kernel takes no dependencies), so this is
/// the tripwire half of the pair: if it fires, move the catalog entries with it.
#[test]
fn notice_spellings_are_what_the_catalog_says_they_are() {
    use crate::NoticeCode;
    let spelled = [
        (NoticeCode::AdvisoryOnly, "advisory_only"),
        (NoticeCode::NoEnvelope, "no_envelope"),
        (NoticeCode::NoIndependentStop, "no_independent_stop"),
        (NoticeCode::UnsourcedLimits, "unsourced_limits"),
        (NoticeCode::GuardUnavailable, "guard_unavailable"),
        (NoticeCode::ProfileWidened, "profile_widened"),
        (NoticeCode::NoRateLimit, "no_rate_limit"),
        (NoticeCode::NoCollisionMargin, "no_collision_margin"),
        (NoticeCode::SessionOverride, "session_override"),
        (NoticeCode::SessionWidened, "session_widened"),
        (NoticeCode::TravelWidened, "travel_widened"),
        (NoticeCode::TravelEmpty, "travel_empty"),
    ];
    for (code, spelling) in spelled {
        assert_eq!(code.as_str(), spelling);
    }
}

// ── §7.1: a travel has two ends (DRY-COVERAGE-PLAN.md F159) ──────────────────

const KNEE_CEILING: [(&str, f64); 1] = [("knee", 2.5)];
const KNEE_IN_POSITION: [(&str, &str); 1] = [("knee", "position")];
const KNEE_TRAVEL: [(&str, f64, f64); 1] = [("knee", 0.0, 2.5)];

fn knee_limits<'a>(
    travel: &'a [(&'a str, f64, f64)],
    joint_travel: &'a [(&'a str, f64, f64)],
) -> ResolvedLimits<'a> {
    resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(TransportClamp {
            per_channel: &KNEE_CEILING,
            joint_modes: &KNEE_IN_POSITION,
            travel,
            ..Default::default()
        }),
        profile: Some(OperatorProfile {
            name: "cell-a",
            max_delta_per_cycle: Some(0.25),
            joint_travel,
            ..Default::default()
        }),
        frames: None,
        available: available(),
    })
}

#[test]
fn a_knee_is_not_commanded_backwards() {
    let desired = [("knee", -1.0)];
    // The control: the magnitude alone, which is all the kernel held before F159. -1 is inside 2.5.
    let magnitude_only = knee_limits(&[], &[]);
    let verdict = evaluate_cycle(&cycle(&desired), &magnitude_only, Some(&envelope()), None);
    assert_eq!(value_of(&verdict, "knee"), Some(-1.0));
    assert!(!verdict.tripped.contains(&Guard::Envelope));

    let limits = knee_limits(&KNEE_TRAVEL, &[]);
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(value_of(&verdict, "knee"), Some(0.0));
    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert_eq!(verdict.response, Some(FaultResponse::Clamp));
    let clamp = verdict
        .clamped
        .iter()
        .next()
        .expect("the clamp is recorded");
    assert_eq!((clamp.from, clamp.to), (-1.0, 0.0));
}

#[test]
fn a_profile_narrows_a_travel_and_is_told_when_it_widens_one() {
    let narrowed = [("knee", 0.2, 1.2)];
    let limits = knee_limits(&KNEE_TRAVEL, &narrowed);
    assert_eq!(limits.travel_of("knee"), Some((0.2, 1.2)));
    assert!(!limits
        .notices
        .iter()
        .any(|notice| notice.code == NoticeCode::TravelWidened));

    let widened = [("knee", -3.0, 3.0)];
    let limits = knee_limits(&KNEE_TRAVEL, &widened);
    assert_eq!(limits.travel_of("knee"), Some((0.0, 2.5)));
    let notice = limits
        .notices
        .iter()
        .find(|notice| notice.code == NoticeCode::TravelWidened)
        .expect("a widening is named, not silently discarded");
    assert_eq!(notice.subjects.len(), 1);
    assert_eq!(notice.subjects.get(0).copied(), Some("knee"));
}

#[test]
fn a_travel_narrowed_to_nothing_refuses_the_start() {
    let disjoint = [("knee", 3.0, 4.0)];
    let limits = knee_limits(&KNEE_TRAVEL, &disjoint);
    assert_eq!(limits.travel_of("knee"), None);
    assert!(limits
        .notices
        .iter()
        .any(|notice| notice.code == NoticeCode::TravelEmpty));
    assert!(can_start(false, &limits, WarningChannel::Ready).is_err());
    assert!(can_start(
        false,
        &knee_limits(&KNEE_TRAVEL, &[]),
        WarningChannel::Ready
    )
    .is_ok());
}

#[test]
fn a_travel_is_held_only_on_a_channel_commanded_in_position() {
    let velocity = [("knee", "velocity")];
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        session: None,
        envelope: Some(envelope()),
        transport: Some(TransportClamp {
            per_channel: &KNEE_CEILING,
            joint_modes: &velocity,
            travel: &KNEE_TRAVEL,
            ..Default::default()
        }),
        profile: Some(profile()),
        frames: None,
        available: available(),
    });
    let desired = [("knee", -1.0)];
    let verdict = evaluate_cycle(&cycle(&desired), &limits, Some(&envelope()), None);
    assert_eq!(value_of(&verdict, "knee"), Some(-1.0));
}

#[test]
fn travel_joints_are_walked_in_utf8_order_whatever_order_the_tables_are_in() {
    let plan = [("wrist", -1.0, 1.0), ("elbow", -2.0, 0.5)];
    let profile = [("knee", 0.0, 1.0), ("elbow", -1.0, 0.0)];
    let limits = knee_limits(&plan, &profile);
    let mut walked: FixedVec<&str, 4> = FixedVec::new();
    let mut after = None;
    while let Some(joint) = limits.next_travel_joint(after) {
        walked.push(joint);
        after = Some(joint);
    }
    assert_eq!(walked.len(), 3);
    for (index, expected) in ["elbow", "knee", "wrist"].iter().enumerate() {
        assert_eq!(walked.get(index), Some(expected));
    }
    assert_eq!(limits.travel_of("elbow"), Some((-1.0, 0.0)));
}
