//! Cross-module behaviour: transport resolution, and how the four contracts compose.
//!
//! The per-module suites check each function in isolation. These are the properties that only show
//! up when the modules meet — a scale computed in `transport` reaching a clamp the kernel enforces,
//! a refused plan reaching an admission decision.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use skynet_json::Json;

use crate::embodiment::*;
use crate::json;
use crate::transport::*;

fn joint(name: &str, role: Option<&str>, effort: Option<f64>, modes: &[&str]) -> EmbodimentJoint {
    EmbodimentJoint {
        name: name.to_string(),
        role: role.map(ToString::to_string),
        joint_type: "revolute".to_string(),
        limits: JointLimits {
            position_min_rad: Some(-3.0),
            position_max_rad: Some(3.0),
            velocity_max_rad_s: Some(2.0),
            effort_max_nm: effort,
        },
        control_modes: modes.iter().map(|m| m.to_string()).collect(),
    }
}

fn body() -> EmbodimentDescriptor {
    EmbodimentDescriptor {
        schema_version: 1,
        robot_slug: "test-arm".to_string(),
        digest: Some("sha256:body".to_string()),
        verified: true,
        base: "fixed".to_string(),
        dof: 3.0,
        joints: vec![
            joint(
                "J1",
                Some("right_arm.shoulder.pitch"),
                Some(80.0),
                &["torque", "position"],
            ),
            joint(
                "J2",
                Some("right_arm.elbow.flex"),
                Some(60.0),
                &["torque", "position"],
            ),
            joint(
                "J3",
                Some("right_arm.wrist.roll"),
                Some(20.0),
                &["position"],
            ),
        ],
        end_effectors: vec![EmbodimentEndEffector {
            name: "hand".to_string(),
            role: Some("right_hand.gripper.grip".to_string()),
            kind: "gripper".to_string(),
            dof: 1.0,
            max_grip_force_n: Some(40.0),
            max_opening_m: Some(0.08),
        }],
        sensors: vec![EmbodimentSensor {
            name: "wrist_cam".to_string(),
            role: Some("right_arm.wrist.roll".to_string()),
            modality: "rgb".to_string(),
            width: Some(640.0),
            height: Some(480.0),
            fps: Some(30.0),
            frame: Some("right_arm.tool0".to_string()),
            latency_ms: Some(28.0),
        }],
        control: ControlProfile {
            modes: vec!["position".to_string(), "torque".to_string()],
            rate_hz_max: Some(200.0),
            rate_hz_min: Some(10.0),
            command_latency_ms: Some(4.0),
        },
        envelope: Some(SafetyEnvelope {
            joint_velocity_max_rad_s: Some(2.0),
            joint_torque_max_nm: Some(80.0),
            base_velocity_max_m_s: None,
            watchdog_ms: Some(100.0),
            estop: Some(true),
            source: Some("https://example.invalid/arm.pdf".to_string()),
        }),
        solvers: vec!["trac_ik".to_string()],
        // An arm with a gripper and a camera, and the tokens a package would ask it for. Written
        // out rather than derived from the effector and the sensor above, which is the whole point
        // of the field: no rule anywhere turns "has a gripper" into `manipulation.grasp`, and
        // inventing one would decide admissions on a mapping nobody specified.
        capabilities: vec![
            "manipulation.grasp".to_string(),
            "sensing.camera.rgb".to_string(),
        ],
        extensions: Vec::new(),
        simulator: false,
        on_command_loss: Some("hold".to_string()),
        watchdog: None,
        power: None,
        frames: None,
    }
}

fn channel(
    index: f64,
    role: Option<&str>,
    joint: Option<&str>,
    peak: Option<f64>,
) -> ActionChannel {
    ActionChannel {
        index,
        kind: "joint_torque".to_string(),
        role: role.map(ToString::to_string),
        joint: joint.map(ToString::to_string),
        unit: Some("Nm".to_string()),
        peak,
        optional: false,
    }
}

fn policy(channels: Vec<ActionChannel>) -> SourceEmbodiment {
    SourceEmbodiment {
        robot_slug: Some("trainer".to_string()),
        digest: Some("sha256:policy".to_string()),
        dof: Some(3.0),
        control_mode: "torque".to_string(),
        control_hz: Some(50.0),
        action_channels: channels,
        observations: Vec::new(),
    }
}

fn codes(plan: &TransportPlan) -> Vec<&'static str> {
    plan.warnings.iter().map(|w| w.code.as_str()).collect()
}

// ── Binding precedence ───────────────────────────────────────────────────────

#[test]
fn an_exact_joint_name_outranks_a_role_match() {
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.elbow.flex"),
        Some("J1"),
        None,
    )]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Direct);
    assert_eq!(plan.bindings[0].target.as_deref(), Some("J1"));
}

#[test]
fn a_role_match_remaps_across_wire_names() {
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.elbow.flex"),
        Some("elbow_joint"),
        None,
    )]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Remap);
    assert_eq!(plan.bindings[0].target.as_deref(), Some("J2"));
    assert!(plan.bindings[0].reason.contains("right_arm.elbow.flex"));
}

#[test]
fn a_required_channel_with_no_equivalent_refuses_the_whole_plan() {
    let source = policy(vec![channel(0.0, Some("left_leg.knee.flex"), None, None)]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Blocked);
    assert_eq!(plan.verdict, TransportVerdict::Refused);
    assert!(!is_installable(&plan));
}

#[test]
fn an_optional_channel_with_no_equivalent_degrades_instead() {
    let mut optional = channel(0.0, Some("left_leg.knee.flex"), None, None);
    optional.optional = true;
    let plan = resolve_transport(&policy(vec![optional]), &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Drop);
    assert_eq!(
        plan.bindings[0].scale, 0.0,
        "a dropped channel commands nothing"
    );
    assert_eq!(plan.verdict, TransportVerdict::Degraded);
    assert!(is_installable(&plan));
}

#[test]
fn a_joint_that_refuses_the_mode_is_blocked_even_though_it_matched() {
    // J3 accepts position only, and the policy emits torque.
    let source = policy(vec![channel(0.0, Some("right_arm.wrist.roll"), None, None)]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Blocked);
    assert_eq!(
        plan.bindings[0].target.as_deref(),
        Some("J3"),
        "the target is still named, so the operator knows which joint"
    );
    assert!(plan.bindings[0].reason.contains("does not accept torque"));
}

// ── Scaling ──────────────────────────────────────────────────────────────────

#[test]
fn a_policy_trained_on_a_stronger_joint_is_scaled_down_and_told_so() {
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.pitch"),
        None,
        Some(180.0),
    )]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].scale, 80.0 / 180.0);
    assert_eq!(plan.bindings[0].limit, Some(80.0));
    assert!(codes(&plan).contains(&"scaled_down"));
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == TransportWarningCode::ScaledDown)
        .unwrap();
    assert!(warning.message.contains("44%"), "{}", warning.message);
}

#[test]
fn a_policy_trained_on_a_weaker_joint_is_not_scaled_up() {
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.pitch"),
        None,
        Some(10.0),
    )]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].scale, 1.0);
    assert!(!codes(&plan).contains(&"scaled_down"));
}

#[test]
fn a_missing_rating_is_reported_rather_than_invented() {
    let mut target = body();
    target.joints[0].limits.effort_max_nm = None;
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.pitch"),
        None,
        Some(180.0),
    )]);
    let plan = resolve_transport(&source, &target, Default::default());
    assert_eq!(plan.bindings[0].scale, 1.0, "no ratio can be computed");
    assert_eq!(plan.bindings[0].limit, None);
    assert!(codes(&plan).contains(&"unscalable_channel"));
}

// ── Retargeting ──────────────────────────────────────────────────────────────

#[test]
fn retargeting_is_off_by_default_and_the_message_says_it_was_not_requested() {
    // A seventh joint on a chain the body has, which is the case task space could rescue.
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.yaw"),
        None,
        None,
    )]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.verdict, TransportVerdict::Refused);
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == TransportWarningCode::RetargetWithoutSolver)
        .unwrap();
    assert!(warning.message.contains("was not requested"));
}

#[test]
fn retargeting_is_granted_when_asked_for_and_a_solver_exists() {
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.yaw"),
        None,
        None,
    )]);
    let plan = resolve_transport(
        &source,
        &body(),
        ResolveTransportOptions {
            allow_retarget: true,
            ..Default::default()
        },
    );
    assert_eq!(plan.bindings[0].method, TransportMethod::Retarget);
    assert_eq!(plan.verdict, TransportVerdict::Retargeted);
}

#[test]
fn retargeting_is_refused_when_the_body_has_no_solver() {
    let mut target = body();
    target.solvers.clear();
    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.yaw"),
        None,
        None,
    )]);
    let plan = resolve_transport(
        &source,
        &target,
        ResolveTransportOptions {
            allow_retarget: true,
            ..Default::default()
        },
    );
    assert_eq!(plan.verdict, TransportVerdict::Refused);
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == TransportWarningCode::RetargetWithoutSolver)
        .unwrap();
    assert!(warning.message.contains("no inverse-kinematics solver"));
}

#[test]
fn retargeting_takes_a_whole_chain_together() {
    // One unmatched joint on the chain; the other two match directly. Every channel on the chain
    // is retargeted, because you cannot send an elbow through task space while its shoulder goes
    // direct.
    let source = policy(vec![
        channel(0.0, Some("right_arm.shoulder.pitch"), None, None),
        channel(1.0, Some("right_arm.shoulder.yaw"), None, None),
    ]);
    let plan = resolve_transport(
        &source,
        &body(),
        ResolveTransportOptions {
            allow_retarget: true,
            ..Default::default()
        },
    );
    // The first still binds by role, because it has an exact role match; the second is the one
    // that had none. The chain-level decision is what *permits* the second, and the specification
    // is explicit that the decision is per chain rather than per joint.
    assert_eq!(plan.bindings[1].method, TransportMethod::Retarget);
    assert!(plan.bindings[1].reason.contains("right_arm chain"));
}

// ── Non-joint channels ───────────────────────────────────────────────────────

#[test]
fn a_gripper_channel_finds_the_end_effector() {
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "gripper".to_string();
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Remap);
    assert_eq!(plan.bindings[0].target.as_deref(), Some("hand"));
    assert_eq!(plan.bindings[0].limit, Some(40.0));
}

#[test]
fn base_velocity_on_a_fixed_base_is_blocked() {
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "base_velocity".to_string();
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Blocked);
}

#[test]
fn base_velocity_onto_a_wheeled_base_is_a_synthesis_not_a_remap() {
    let mut target = body();
    target.base = "wheeled".to_string();
    target.envelope.as_mut().unwrap().base_velocity_max_m_s = Some(1.2);
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "base_velocity".to_string();
    let plan = resolve_transport(&source, &target, Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Synthesize);
    assert_eq!(plan.bindings[0].limit, Some(1.2));
    assert_eq!(plan.verdict, TransportVerdict::Retargeted);
}

#[test]
fn base_velocity_between_two_legged_bodies_is_a_remap() {
    let mut target = body();
    target.base = "legged".to_string();
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "base_velocity".to_string();
    let plan = resolve_transport(&source, &target, Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Remap);
}

#[test]
fn a_discrete_channel_passes_through_unbounded() {
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "discrete".to_string();
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.bindings[0].method, TransportMethod::Direct);
    assert_eq!(
        plan.bindings[0].limit, None,
        "it commands no actuator directly"
    );
}

#[test]
fn a_cartesian_channel_needs_both_an_effector_and_a_solver() {
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "cartesian".to_string();
    assert_eq!(
        resolve_transport(&source, &body(), Default::default()).bindings[0].method,
        TransportMethod::Retarget
    );

    let mut no_solver = body();
    no_solver.solvers.clear();
    assert_eq!(
        resolve_transport(&source, &no_solver, Default::default()).bindings[0].method,
        TransportMethod::Blocked
    );
}

// ── Observations ─────────────────────────────────────────────────────────────

#[test]
fn an_exact_sensor_match_binds_directly() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    source.observations.push(ObservationRequirement {
        name: "wrist_cam".to_string(),
        modality: "rgb".to_string(),
        role: None,
        width: Some(640.0),
        height: Some(480.0),
        fps: Some(30.0),
        optional: false,
    });
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.observations[0].method, ObservationMethod::Direct);
}

#[test]
fn a_poorer_sensor_downscales_and_says_so() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    source.observations.push(ObservationRequirement {
        name: "wrist_cam".to_string(),
        modality: "rgb".to_string(),
        role: None,
        width: Some(1920.0),
        height: Some(1080.0),
        fps: Some(60.0),
        optional: false,
    });
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.observations[0].method, ObservationMethod::Downscale);
    assert!(codes(&plan).contains(&"observation_degraded"));
}

#[test]
fn a_missing_required_stream_warns_and_a_missing_optional_one_does_not() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    source.observations.push(ObservationRequirement {
        name: "head_cam".to_string(),
        modality: "depth".to_string(),
        role: None,
        width: None,
        height: None,
        fps: None,
        optional: false,
    });
    assert!(
        codes(&resolve_transport(&source, &body(), Default::default()))
            .contains(&"observation_missing")
    );

    source.observations[0].optional = true;
    let plan = resolve_transport(&source, &body(), Default::default());
    assert!(!codes(&plan).contains(&"observation_missing"));
    assert_eq!(plan.observations[0].method, ObservationMethod::Missing);
}

// ── Rate ─────────────────────────────────────────────────────────────────────

#[test]
fn the_loop_clamps_down_to_what_the_body_accepts_never_up() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    source.control_hz = Some(1000.0);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.control_hz, Some(200.0));
    assert!(codes(&plan).contains(&"rate_unsupported"));

    source.control_hz = Some(20.0);
    let slow = resolve_transport(&source, &body(), Default::default());
    assert_eq!(slow.control_hz, Some(20.0), "never clamped upward");
}

#[test]
fn a_loop_slower_than_the_watchdog_is_the_warning_that_matters_most() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    // A 100 ms watchdog needs at least 10 Hz.
    source.control_hz = Some(5.0);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert!(codes(&plan).contains(&"rate_below_watchdog"));
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == TransportWarningCode::RateBelowWatchdog)
        .unwrap();
    assert!(warning.message.contains("100 ms watchdog"));
}

// ── The declaration warnings ─────────────────────────────────────────────────

#[test]
fn an_unverified_body_is_usable_and_flagged() {
    let mut target = body();
    target.verified = false;
    let source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    let plan = resolve_transport(&source, &target, Default::default());
    assert_eq!(plan.verdict, TransportVerdict::Direct, "still usable");
    assert!(codes(&plan).contains(&"unverified_embodiment"));
}

#[test]
fn a_body_with_no_envelope_still_gets_per_joint_ceilings() {
    let mut target = body();
    target.envelope = None;
    let source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    let plan = resolve_transport(&source, &target, Default::default());
    assert!(codes(&plan).contains(&"no_target_envelope"));
    let clamp = plan.clamp.as_ref().unwrap();
    assert_eq!(clamp.joint_torque_max_nm, None);
    assert_eq!(
        clamp
            .per_channel
            .iter()
            .find(|(k, _)| k == "J2")
            .map(|(_, v)| *v),
        Some(60.0),
        "the joint's own rating survives even with no whole-body envelope"
    );
}

#[test]
fn a_body_with_neither_an_envelope_nor_a_bound_channel_has_no_clamp_at_all() {
    let mut target = body();
    target.envelope = None;
    let mut source = policy(vec![channel(0.0, None, None, None)]);
    source.action_channels[0].kind = "discrete".to_string();
    let plan = resolve_transport(&source, &target, Default::default());
    assert_eq!(plan.clamp, None, "not an empty clamp — no clamp");
}

#[test]
fn an_undeclared_command_latency_is_a_warning() {
    let mut target = body();
    target.control.command_latency_ms = None;
    let source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    assert!(
        codes(&resolve_transport(&source, &target, Default::default()))
            .contains(&"latency_undeclared")
    );
}

#[test]
fn a_dof_mismatch_is_reported_with_both_numbers() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    source.dof = Some(7.0);
    let plan = resolve_transport(&source, &body(), Default::default());
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == TransportWarningCode::DofMismatch)
        .unwrap();
    assert!(warning.message.contains("7 degree-of-freedom"));
    assert!(warning.message.contains("this one has 3"));
}

#[test]
fn a_mode_the_body_does_not_accept_is_reported() {
    let mut source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    source.control_mode = "trajectory".to_string();
    assert!(
        codes(&resolve_transport(&source, &body(), Default::default()))
            .contains(&"control_mode_unsupported")
    );
}

#[test]
fn joints_the_policy_does_not_drive_are_named() {
    let source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    let plan = resolve_transport(&source, &body(), Default::default());
    let warning = plan
        .warnings
        .iter()
        .find(|w| w.code == TransportWarningCode::UnmappedRoles)
        .unwrap();
    let subjects = warning.subjects.as_ref().unwrap();
    assert!(subjects.contains(&"right_arm.shoulder.pitch".to_string()));
    assert!(subjects.contains(&"right_arm.wrist.roll".to_string()));
    assert!(!subjects.contains(&"right_arm.elbow.flex".to_string()));
}

#[test]
fn an_empty_action_space_says_nothing_can_be_checked() {
    let plan = resolve_transport(&policy(Vec::new()), &body(), Default::default());
    assert!(codes(&plan).contains(&"no_source_embodiment"));
    assert_eq!(plan.verdict, TransportVerdict::Direct, "vacuously");
}

// ── Verdict derivation ───────────────────────────────────────────────────────

#[test]
fn the_verdict_is_the_worst_binding_in_the_plan() {
    let mut optional = channel(2.0, Some("left_leg.knee.flex"), None, None);
    optional.optional = true;
    let source = policy(vec![
        channel(0.0, Some("right_arm.shoulder.pitch"), None, None),
        optional,
    ]);
    let plan = resolve_transport(&source, &body(), Default::default());
    assert_eq!(plan.verdict, TransportVerdict::Degraded);

    // Adding one blocked required channel takes the whole plan to refused.
    let mut with_blocked = source.clone();
    with_blocked
        .action_channels
        .push(channel(3.0, Some("left_leg.ankle.pitch"), None, None));
    assert_eq!(
        resolve_transport(&with_blocked, &body(), Default::default()).verdict,
        TransportVerdict::Refused
    );
}

// ── Composition with the other contracts ─────────────────────────────────────

#[test]
fn a_refused_plan_refuses_the_install() {
    use crate::admission::*;

    let params = AdmissionParams {
        manifest: PackageManifest {
            schema_version: 1,
            id: "acme/grasp".to_string(),
            version: "1.0.0".to_string(),
            kind: "policy".to_string(),
            digest: "sha256:pkg".to_string(),
            isolation: "sandboxed".to_string(),
            budget: ResourceBudget {
                ram_mb: 512.0,
                ..Default::default()
            },
            embodiment: EmbodimentRequirement {
                control_mode: "torque".to_string(),
                ..Default::default()
            },
            requests: Vec::new(),
            runtime: None,
            depends_on: Vec::new(),
            min_os_version: None,
        },
        capacity: DeviceCapacity {
            ram_mb: 8192.0,
            os_version: "1.0.0".to_string(),
            ..Default::default()
        },
        signature: SignatureState::Verified,
        observed_digest: "sha256:pkg".to_string(),
        transport_verdict: Some("refused".to_string()),
        executes_on_load: false,
        trust_remote_code: false,
    };

    let result = admit_package(&params);
    assert!(!result.admitted);
    assert!(result
        .findings
        .iter()
        .any(|f| f.code == PackageRejectionCode::TransportRefused));
    assert!(
        result.effective_grants.is_empty(),
        "a refused install grants nothing"
    );
}

#[test]
fn a_transport_clamp_becomes_the_ceiling_the_kernel_enforces() {
    use crate::safety::*;

    let source = policy(vec![channel(
        0.0,
        Some("right_arm.shoulder.pitch"),
        None,
        Some(180.0),
    )]);
    let plan = resolve_transport(&source, &body(), Default::default());
    let clamp = plan.clamp.as_ref().unwrap();

    // Hand the plan's per-channel ceilings to the kernel, exactly as the runtime would.
    let per_channel: Vec<(&str, f64)> = clamp
        .per_channel
        .iter()
        .map(|(name, limit)| (name.as_str(), *limit))
        .collect();
    let kernel_clamp = TransportClamp {
        joint_torque_max_nm: clamp.joint_torque_max_nm,
        per_channel: &per_channel,
        ..Default::default()
    };
    let envelope = Envelope {
        watchdog_ms: Some(100.0),
        ..Default::default()
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        envelope: Some(envelope),
        transport: Some(kernel_clamp),
        profile: None,
        frames: None,
        available: Available::default(),
    });

    let desired = [("J1", 150.0)];
    let cycle = CommandCycle {
        now_ms: 0.0,
        command_ms: 0.0,
        sequence: 1,
        desired: &desired,
        previous: None,
        capability: None,
        nearest_person_m: None,
    };
    let verdict = evaluate_cycle(&cycle, &limits, Some(&envelope), None);
    assert_eq!(
        verdict.command.get(0).map(|(_, v)| *v),
        Some(80.0),
        "the planner decided the ceiling and the kernel enforced it"
    );
}

// ── Determinism ──────────────────────────────────────────────────────────────

#[test]
fn the_same_inputs_produce_byte_identical_output() {
    let source = policy(vec![
        channel(0.0, Some("right_arm.shoulder.pitch"), None, Some(180.0)),
        channel(1.0, Some("right_arm.elbow.flex"), None, None),
    ]);
    let first = crate::json::plan_to_json(&resolve_transport(&source, &body(), Default::default()))
        .to_canonical()
        .unwrap();
    let second =
        crate::json::plan_to_json(&resolve_transport(&source, &body(), Default::default()))
            .to_canonical()
            .unwrap();
    assert_eq!(first, second);
}

#[test]
fn bindings_come_back_in_the_policys_output_order() {
    let source = policy(vec![
        channel(0.0, Some("right_arm.wrist.roll"), None, None),
        channel(1.0, Some("right_arm.shoulder.pitch"), None, None),
        channel(2.0, Some("right_arm.elbow.flex"), None, None),
    ]);
    let plan = resolve_transport(&source, &body(), Default::default());
    let indices: Vec<f64> = plan.bindings.iter().map(|b| b.source_index).collect();
    assert_eq!(indices, vec![0.0, 1.0, 2.0]);
}

#[test]
fn a_plan_serialises_to_a_stable_shape() {
    let source = policy(vec![channel(0.0, Some("right_arm.elbow.flex"), None, None)]);
    let plan = resolve_transport(&source, &body(), Default::default());
    let json = crate::json::plan_to_json(&plan);
    for key in [
        "verdict",
        "robot_slug",
        "source_digest",
        "target_digest",
        "control_mode",
        "control_hz",
        "bindings",
        "observations",
        "clamp",
        "warnings",
    ] {
        assert!(
            json.get(key).is_some(),
            "missing `{key}` in the serialised plan"
        );
    }
    let text: String = json.to_canonical().unwrap();
    assert!(text.starts_with('{') && text.ends_with('}'));
}

// ── The frame graph, §44.3 ───────────────────────────────────────────────────
//
// The corpus holds this edition to the oracle's answers case by case. These are the edition's own
// tests of the decoder around it, which the corpus cannot reach: the corpus feeds a graph, and the
// question here is what a *descriptor* carrying one, or not carrying one, decodes to.

#[test]
fn a_descriptor_with_no_frames_decodes_to_none() {
    let json = Json::parse(
        r#"{"schema_version":1,"robot_slug":"arm","base":"fixed","dof":1,"joints":[]}"#,
    )
    .unwrap();
    let descriptor = json::descriptor_from_json(&json).unwrap();
    assert!(descriptor.frames.is_none());
    assert!(!descriptor.simulator, "absent means not a simulator");
}

#[test]
fn a_null_frame_graph_is_the_same_fact_as_an_absent_one() {
    // Different provenance — a generator that declined versus a schema that predates the field —
    // and the same consequence: there is no tree to compute kinematics from. §44.5's failure is the
    // same either way, so the decoder does not manufacture a distinction it cannot act on.
    let json = Json::parse(
        r#"{"schema_version":1,"robot_slug":"arm","base":"fixed","dof":1,"joints":[],"frames":null}"#,
    )
    .unwrap();
    assert!(json::descriptor_from_json(&json).unwrap().frames.is_none());
}

#[test]
fn a_frame_graph_decodes_field_by_field() {
    let json = Json::parse(
        r#"{"schema_version":1,"robot_slug":"arm","base":"fixed","dof":1,"joints":[],
            "simulator":true,
            "frames":{"root":"base_link","convention":"rep103","generator":"mjcf","nodes":[
              {"name":"base_link","parent":null,"kind":"root","collision":true},
              {"name":"link1","parent":"base_link","kind":"dynamic","joint":"j1",
               "joint_type":"revolute","axis":[0,1,0],"translation":[0,0,0.1],
               "rotation":[0,0,0,1],"source":"descriptor","collision":true,"mass_kg":1.5}]}}"#,
    )
    .unwrap();
    let descriptor = json::descriptor_from_json(&json).unwrap();
    assert!(descriptor.simulator);

    let graph = descriptor.frames.expect("a graph was declared");
    assert_eq!(graph.root, "base_link");
    assert_eq!(graph.generator.as_deref(), Some("mjcf"));
    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.nodes[0].parent.is_none());
    assert_eq!(graph.nodes[1].axis, Some([0.0, 1.0, 0.0]));
    assert_eq!(graph.nodes[1].rotation, Some([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(graph.nodes[1].mass_kg, Some(1.5));
    assert!(validate_frames(&graph).is_none());
}

#[test]
fn a_vector_of_the_wrong_length_is_absent_rather_than_padded() {
    // A three-element rotation is not a quaternion missing a component; it is a field somebody got
    // wrong, and inventing the fourth number would produce a rotation nobody wrote.
    let json = Json::parse(
        r#"{"schema_version":1,"robot_slug":"arm","base":"fixed","dof":1,"joints":[],
            "frames":{"root":"a","convention":"rep103","nodes":[
              {"name":"a","parent":null,"kind":"root"},
              {"name":"b","parent":"a","kind":"static","rotation":[0,0,1]}]}}"#,
    )
    .unwrap();
    let graph = json::descriptor_from_json(&json).unwrap().frames.unwrap();
    assert_eq!(graph.nodes[1].rotation, None);
}

/// §49.1's warning channel, which is the safety-relevant reason §49 exists at all.
///
/// > `skynetd` owns it, it sounds on transition into `RUNNING` and on resumption after a stop, and
/// > **a package cannot suppress it.** A policy that could silence its own warning would be a
/// > policy that could remove a control the operator's risk assessment relied on.
mod the_warning_channel {
    use crate::admission::*;
    use alloc::string::ToString;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A package that would otherwise be admitted, so a refusal below is about the token.
    fn params(capabilities: Vec<&str>, requests: Vec<&str>) -> AdmissionParams {
        AdmissionParams {
            manifest: PackageManifest {
                schema_version: 1,
                id: "acme/chatty".to_string(),
                version: "1.0.0".to_string(),
                kind: "policy".to_string(),
                digest: "sha256:pkg".to_string(),
                isolation: "sandboxed".to_string(),
                budget: ResourceBudget {
                    ram_mb: 512.0,
                    ..Default::default()
                },
                embodiment: EmbodimentRequirement {
                    control_mode: "torque".to_string(),
                    capabilities: capabilities.iter().map(|t| t.to_string()).collect(),
                    ..Default::default()
                },
                requests: requests.iter().map(|t| t.to_string()).collect(),
                runtime: None,
                depends_on: Vec::new(),
                min_os_version: None,
            },
            capacity: DeviceCapacity {
                ram_mb: 8192.0,
                os_version: "1.0.0".to_string(),
                // The robot provides everything asked for, and the operator has granted it. The
                // point of the tests below is that this is not enough.
                capabilities: capabilities.iter().map(|t| t.to_string()).collect(),
                granted: requests.iter().map(|t| t.to_string()).collect(),
                ..Default::default()
            },
            signature: SignatureState::Verified,
            observed_digest: "sha256:pkg".to_string(),
            transport_verdict: None,
            executes_on_load: false,
            trust_remote_code: false,
        }
    }

    fn codes(result: &AdmissionResult) -> Vec<PackageRejectionCode> {
        result.findings.iter().map(|f| f.code).collect()
    }

    #[test]
    fn a_package_granted_everything_it_asked_for_still_cannot_have_the_warning_channel() {
        // The robot provides it and the operator granted it, and it is still refused — which is
        // what "no grant confers it" has to mean if it means anything.
        let result = admit_package(&params(vec![WARNING_CHANNEL], vec![WARNING_CHANNEL]));
        assert!(!result.admitted);
        assert!(codes(&result).contains(&PackageRejectionCode::CapabilityReserved));
        assert!(
            result.effective_grants.is_empty(),
            "a refused install grants nothing, least of all this"
        );
    }

    #[test]
    fn declaring_a_need_for_it_is_refused_even_without_requesting_it() {
        // Both halves, because a package that declared the capability and never requested the grant
        // would otherwise install and be one manifest edit away from claiming the channel.
        let result = admit_package(&params(vec![WARNING_CHANNEL], vec![]));
        assert!(!result.admitted);
        assert!(codes(&result).contains(&PackageRejectionCode::CapabilityReserved));
    }

    #[test]
    fn hri_speech_is_ordinary_and_is_admitted() {
        // §49.1 gives packages the speaker for everything else. A rule that refused audio outright
        // would be a rule nobody could ship an HRI package under, and vendors would route around
        // the whole capability system.
        let result = admit_package(&params(vec!["hri.speech"], vec!["hri.speech"]));
        assert!(result.admitted, "{:?}", result.findings);
        assert!(result.effective_grants.contains(&"hri.speech".to_string()));
    }

    #[test]
    fn a_reserved_token_is_not_reported_as_denied() {
        // The distinction that decides where a person goes. "Denied" is an operator's decision and
        // reversible; this is not grantable by anybody, and an operator told "denied" would go
        // hunting for a setting that does not exist.
        let result = admit_package(&params(vec![WARNING_CHANNEL], vec![WARNING_CHANNEL]));
        assert!(!codes(&result).contains(&PackageRejectionCode::CapabilityDenied));
        let finding = result
            .findings
            .iter()
            .find(|f| f.code == PackageRejectionCode::CapabilityReserved)
            .expect("the finding");
        assert!(finding.message.contains("no grant confers it"));
        assert!(finding.message.contains("silence its own warning"));
        assert_eq!(
            finding.subjects.as_ref().map(Vec::len),
            Some(1),
            "the finding names the token"
        );
    }

    #[test]
    fn the_reserved_list_is_the_single_place_the_rule_lives() {
        assert!(is_reserved(WARNING_CHANNEL));
        assert!(!is_reserved("hri.speech"));
        assert!(!is_reserved("sensing.audio"));
        assert!(RESERVED_CAPABILITIES.contains(&WARNING_CHANNEL));
    }

    // §49.1's transition half moved to `crate::opstate` when §27's machine was declared, and the
    // three tests that lived here went with it — including the one whose own comment said it "is
    // the one that should be deleted" once the states stopped being prose. What it was protecting
    // was case-insensitive string comparison, which a typed transition has no use for.
    //
    // One of the assertions it carried was `!warning_required("hold", "degraded")`, frozen as
    // correct. It was not. See `opstate.rs`.
}
