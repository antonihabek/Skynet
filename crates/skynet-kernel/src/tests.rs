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
    }
}

const PER_CHANNEL: [(&str, f64); 2] = [("elbow", 80.0), ("shoulder", 60.0)];

fn clamp() -> TransportClamp<'static> {
    TransportClamp {
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: Some(80.0),
        base_velocity_max_m_s: Some(1.5),
        min_control_hz: Some(10.0),
        per_channel: &PER_CHANNEL,
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
        collision_model: true,
        solvers: true,
        legged: true,
    }
}

fn host_limits() -> ResolvedLimits<'static> {
    resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(narrow_profile),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(greedy),
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
        envelope: None,
        transport: Some(clamp()),
        profile: Some(profile()),
        available: available(),
    });
    assert_eq!(limits.joint_torque_max_nm, Some(80.0));
}

#[test]
fn a_body_with_no_envelope_says_so_rather_than_assuming_one() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        envelope: None,
        transport: None,
        profile: None,
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
        envelope: Some(unsourced),
        transport: None,
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        available: Available {
            collision_model: false,
            ..available()
        },
    });
    assert!(!limits.is_active(Guard::SelfCollision));
    let entry = limits
        .unavailable_guards
        .iter()
        .find(|u| u.guard == Guard::SelfCollision)
        .expect("reported, with the missing input named");
    assert!(entry
        .missing
        .contains(&"a collision model in the body description"));
}

#[test]
fn balance_on_a_wheeled_base_reads_as_inapplicable_not_broken() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: None,
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
        envelope: None,
        transport: None,
        profile: None,
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
        envelope: Some(no_window),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
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
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(OperatorProfile {
            proximity: Some(prox),
            ..profile()
        }),
        available: available(),
    });
    let desired = [("elbow", 500.0)];
    let mut c = cycle(&desired);
    c.nearest_person_m = Some(0.8);
    let verdict = evaluate_cycle(&c, &limits, Some(&envelope()), None);
    // Envelope wants `clamp`; proximity wants `stop_cat1`. The stronger wins.
    assert!(verdict.tripped.contains(&Guard::Envelope));
    assert!(verdict.tripped.contains(&Guard::Proximity));
    assert_eq!(verdict.response, Some(FaultResponse::StopCat1));
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
    assert!(can_start(true, &host_limits()).is_err());
}

#[test]
fn a_tier_that_promised_enforcement_and_resolved_to_advisory_cannot_start() {
    // Not reachable through `resolve_limits`, which derives enforcement — so this is a check on
    // a limit set assembled some other way, which is exactly when it is worth having.
    let mut limits = host_limits();
    limits.enforcement = Enforcement::Advisory;
    assert!(can_start(false, &limits).is_err());
}

#[test]
fn a_guest_tier_robot_starts_despite_advisory_enforcement() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Guest,
        envelope: Some(envelope()),
        transport: Some(clamp()),
        profile: Some(profile()),
        available: available(),
    });
    assert!(can_start(false, &limits).is_ok());
}

#[test]
fn notices_and_imperfections_do_not_block_a_start() {
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        envelope: None,
        transport: None,
        profile: None,
        available: Available::default(),
    });
    assert!(
        can_start(false, &limits).is_ok(),
        "a system that refuses to start on every imperfection is a system that gets bypassed"
    );
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
