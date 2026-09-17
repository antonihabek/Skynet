// The closed string sets — `corpus/vocabulary/closed-sets.json`, and the only contract in this edition
// with no module of its own in `reference/typescript`.
//
// Every other corpus holds an edition to a *computation*: give it this input, emit these bytes. This one
// holds it to a **vocabulary**, and the generator's own comment says why it exists: `capability_reserved`
// was added to the Rust `PackageRejectionCode` and not to the reference's, and nothing could see it — the
// mirror pins guard the reference, and every corpus covers the cases it holds, none of which had ever
// mentioned a token invented ten minutes earlier. A missing member is now a corpus failure on the first
// run, with the set named.
//
// Seventy-four sets, and **order is part of the agreement** for several of them: `os_tiers` and
// `scts_areas` are ranked, `operating_states` and `state_reasons` run in the order `evaluate` descends
// through its rules, and `skill_phases` runs in diagram order so that "never moves backwards through this
// list" is a property somebody can assert. An edition that sorted any of those alphabetically would pass
// every other corpus and would have lost the property.
//
// The members' order is frozen output and a `List` promises it. The *sets'* order is not output — it only
// decides the order the self-check walks them in — so the `LinkedHashMap` below is for a readable table
// and a readable failure rather than for correctness, and this says so instead of leaving a reader to
// assume the stronger claim.
//
// The table is this edition's own statement of what it believes the vocabulary to be, written out rather
// than generated into every edition from one source — a token added to one edition and not another is the
// divergence the corpus exists to catch, and it cannot catch one that a generator put in all of them.

package skynet.os.contracts;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import skynet.os.Value;

public final class Vocabulary {
    private Vocabulary() {}

    /**
     * The sets, as they are declared in {@code reference/typescript} — one exported constant per set,
     * resolved through the {@code vocabulary()} calls in {@code tools/corpus-gen/generate.ts} that name
     * which constant each set is.
     *
     * <p>Built in a static initialiser through {@code put} rather than written as a map literal, because
     * Java has none — and because {@code List.of(...)} refuses a null, so a member accidentally left out
     * of the generator's reading would be a compile error rather than a silent hole.
     */
    private static final Map<String, List<String>> SETS = new LinkedHashMap<>();

    private static void put(String name, String... members) {
        // A duplicate name would replace the earlier set in place and lose it, so it is refused here
        // rather than found later by a reader wondering which of two lists is the answer.
        if (SETS.put(name, List.of(members)) != null) {
            throw new IllegalStateException("`" + name + "` is declared twice");
        }
    }

    static {
        put("package_rejection_codes", "unsigned", "signature_failed", "signature_untrusted",
                "signature_unverifiable", "digest_mismatch", "isolation_violation", "budget_exceeded",
                "capability_ungranted", "capability_denied", "capability_reserved", "over_request",
                "missing_dependency", "os_too_old", "transport_refused", "executes_on_load_untrusted",
                "environment_inert", "embodiment_undeclared", "capability_unknown");
        put("signature_states", "verified", "failed", "untrusted", "absent", "unverifiable");
        put("package_kinds", "policy", "skill", "driver", "service", "embodiment", "environment");
        put("isolation_modes", "sandboxed", "native");
        put("reserved_capabilities", "hri.warning");
        put("capability_domains", "locomotion", "manipulation", "sensing", "control", "safety", "platform", "hri");
        // §85.1, in the order the section lists them. Membership is the whole rule, and not
        // every token has two segments — `sensing.camera.rgb` has three.
        put("capability_tokens",
                "locomotion.walk", "locomotion.velocity", "locomotion.stairs", "locomotion.turn_in_place",
                "locomotion.stand", "locomotion.fly", "manipulation.arm", "manipulation.dual_arm",
                "manipulation.grasp", "manipulation.dexterous_hand", "manipulation.cartesian",
                "manipulation.force_control", "manipulation.mobile", "sensing.camera.rgb",
                "sensing.camera.depth", "sensing.camera.stereo", "sensing.lidar", "sensing.imu",
                "sensing.force_torque", "sensing.tactile", "sensing.joint_state", "sensing.odometry",
                "sensing.audio", "control.position", "control.velocity", "control.torque", "control.trajectory",
                "control.realtime", "control.motion", "safety.estop", "safety.collision_avoidance",
                "safety.workspace_limits", "platform.onboard_compute", "platform.ros2", "platform.teleop",
                "platform.data_recording", "hri.speech", "hri.warning");
        put("token_classes", "canonical", "unknown", "vendor", "malformed");
        put("transport_verdicts", "direct", "retargeted", "degraded", "refused");
        put("transport_methods", "direct", "remap", "retarget", "synthesize", "drop", "blocked");
        put("transport_warning_codes", "no_source_embodiment", "unverified_embodiment", "no_target_envelope",
                "dof_mismatch", "control_mode_unsupported", "rate_unsupported", "rate_below_watchdog",
                "scaled_down", "unscalable_channel", "retarget_without_solver", "unmapped_roles",
                "observation_missing", "observation_degraded", "latency_undeclared", "action_space_mismatch",
                "action_space_ambiguous", "motion_parameter_unused",
                // §86, appended for the reason every other addition here is.
                "joint_mode_conflict", "mixed_modes_unsupported");
        put("action_kinds", "joint_position", "joint_velocity", "joint_torque", "cartesian", "base_velocity",
                "gripper", "discrete");
        put("control_modes", "position", "velocity", "torque", "trajectory", "cartesian");
        put("joint_types", "revolute", "prismatic", "continuous", "fixed");
        put("base_kinds", "fixed", "wheeled", "tracked", "legged", "aerial", "rail");
        put("embodiment_modalities", "rgb", "depth", "stereo", "point_cloud", "imu", "force_torque", "tactile",
                "audio", "joint_state");
        put("canonical_chains", "left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
                "torso", "neck", "head", "base", "tail", "aux");
        put("canonical_axes", "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip");
        put("canonical_motions", "move_joint", "move_linear", "move_circular", "grasp", "release", "home", "wait",
                "tool_change", "io_set");
        put("motion_fault_codes", "unnamed", "duplicate_motion", "unknown_motion", "duplicate_parameter",
                "required_undeclared", "bound_invalid");
        put("os_tiers", "guest", "host", "certified");
        put("enforcement_modes", "advisory", "enforcing", "enforcing_redundant");
        put("guards", "watchdog", "envelope", "rate_limit", "workspace", "self_collision", "balance", "proximity",
                "mode_gate");
        put("fault_responses", "clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0");
        put("scts_suites", "sim", "bench", "field");
        put("scts_areas", "rhal", "embodiment", "safety", "package", "ota", "telemetry", "offline", "security",
                "power");
        put("predicate_ops", "and", "or", "not", "eq", "neq", "in", "gte", "lte", "gt", "lt", "includes",
                "includes_all", "includes_any", "exists", "semver_satisfies");
        put("predicate_outcomes", "satisfied", "unsatisfied", "unevaluable");
        put("unevaluable_reasons", "malformed", "unknown_op", "missing_path", "missing_operand", "unresolved_path",
                "not_a_number", "not_a_version");
        put("clause_outcomes", "satisfied", "unsatisfied", "unevaluable", "unenforceable");
        put("clause_origins", "skill", "operator");
        put("precondition_phases", "entry", "hold");
        put("skill_outcomes", "success", "failure", "aborted", "unlabeled");
        put("composition_faults", "cycle", "too_deep");
        put("cell_faults", "unnamed", "fact_key_unusable", "fact_key_reserved", "volume_frame_missing",
                "volume_shape_invalid", "fixture_unnamed", "duplicate_fixture", "parking_channel_unnamed",
                "duplicate_parking_channel",
                // §87.1, and all five apply only from the second robot.
                "robot_unnamed", "duplicate_robot", "robot_unplaced", "robot_unbounded", "shared_volume");
        // §87.2. `live` is deliberately not a basis: a vendor who writes it has proposed the design that
        // subsection rejected, so `basis_refused` names it rather than reporting an unrecognised word.
        put("safe_state_bases", "fixed", "mapped_worst_case");
        put("safe_state_faults", "basis_undeclared", "basis_refused", "basis_unknown", "area_undeclared",
                "mapped_on_a_fixed_base", "figure_undeclared");
        // §87.4. A route is about latency and a role is about authority, which is why they are two sets.
        put("relay_routes", "lan", "relay");
        put("console_roles", "anonymous", "observer", "operator", "admin");
        put("console_actions", "stop_request", "read_status", "read_health", "start_package", "stop_package",
                "grant_capability", "revoke_capability", "seal_blackbox", "apply_update", "pair",
                "install_driver", "change_profile", "factory_reset");
        put("session_states", "live", "expired", "refused");
        put("session_refusals", "deadline_undeclared", "route_unknown", "role_unknown");
        put("action_refusals", "role_insufficient", "presence_required", "session_expired", "session_refused");
        // §88.2, in the order they are evaluated: authority, then state, then content.
        put("motion_refusals", "no_lease", "estop", "not_active", "busy", "unknown_motion",
                "deadline_undeclared", "parameter_unknown", "parameter_missing", "parameter_not_finite",
                "parameter_out_of_bounds");
        put("motion_ready_states", "running", "degraded");
        put("motion_progress", "running", "overdue");
        put("operating_states", "safe", "stopping", "post", "unhomed", "hold", "degraded", "running");
        put("state_reasons", "rhal_lost", "safe_sticky", "post_failed", "stop_complete", "stopping_watchdog",
                "stopping_proximity", "stopping_balance", "stopping_operator", "stopping_estop", "post_incomplete",
                "homing_pending", "kernel_down", "guard_hold", "no_policy", "policy_not_ready",
                "guards_unavailable", "rhal_degraded", "nominal");
        put("stop_causes", "watchdog", "proximity", "balance", "operator", "estop");
        put("post_verdicts", "passed", "failed", "incomplete");
        put("homing_states", "not_required", "pending", "complete");
        put("rhal_states", "present", "degraded", "lost");
        put("policy_states", "none", "held", "ready");
        put("kernel_states", "live", "down");
        put("skill_phases", "idle", "preconditions", "acquiring", "running", "verifying", "terminal");
        put("lifecycle_reasons", "not_invoked", "invoked", "preconditions_pending", "preconditions_refused",
                "preconditions_met", "leases_held_by_root", "lease_pending", "lease_denied", "power_withheld",
                "leases_acquired", "timeout", "hold_lost", "guard_tripped", "robot_stopped", "running", "verifying",
                "verdict_pending", "judged", "terminal");
        put("lease_states", "pending", "held", "denied");
        put("os_components", "bootloader", "kernel_image", "rhal_abi", "rhal_driver", "safety_kernel", "envelope",
                "skynetd", "policy_runtime", "userspace");
        put("certification_gated", "safety_kernel", "envelope");
        put("bundle_faults", "unversioned", "undigested", "signature_failed", "signature_untrusted",
                "signature_absent", "signature_unverifiable", "incompatible", "compatibility_undeclared",
                "channel_mismatch", "certification_required", "bench_pass_stale", "bench_pass_failed",
                "already_running");
        put("boot_decisions", "not_pending", "wait", "confirm", "roll_back");
        put("code_faults", "empty", "looks_like_a_credential", "too_short", "too_long", "not_in_alphabet");
        put("device_scopes", "device:calibration", "device:checkin", "device:conformance", "device:desired",
                "device:episodes", "device:evaluations", "device:incidents", "device:releases");
        put("key_bindings", "hardware", "software");
        put("identity_findings", "not_paired", "namespace_missing", "not_a_device_credential", "scope_unrecognised",
                "scope_undeclared", "key_binding_unknown", "key_not_hardware_bound", "key_not_attested",
                "no_birth_certificate");
        put("measure_states", "known", "nothing", "unmeasured");
        put("birth_findings", "unattested", "device_key_mismatch", "serial_mismatch", "descriptor_mismatch",
                "hardware_revision_mismatch", "ownership_asserted", "unobservable", "incomplete");
        put("birth_required_fields", "device_key", "model", "serial", "descriptor_digest", "hardware_revision",
                "manufactured");
        put("birth_ownership_fields", "namespace", "owner", "operator", "customer");
        put("provenance_classes", "field", "simulated", "unattested");
        put("provenance_reason_codes", "device_signature", "device_unknown", "simulator_declared",
                "birth_unattested", "image_unattested", "seeds_not_supplied", "benchmark_mismatch", "not_replayable");
        put("checkin_claims", "applied", "converging", "deferred", "inconsistent");
        put("reconcile_stages", "os_image", "rhal_driver", "package", "profile", "policy", "lease");
        put("readiness_states", "ready", "stalled", "not_running", "unknown");
        put("health_fields", "generation_lag", "guard_trip_rate", "storage_headroom", "battery", "temperature_c");
        put("offer_faults", "undecodable", "generation_reused", "generation_went_backwards", "empties_the_robot");
        put("telemetry_counters", "cycles", "guard_trips", "clamps", "interventions", "estops", "package_restarts",
                "queue_dropped", "reconcile_passes");
        put("power_states", "normal", "low", "reserve", "critical", "unmetered");
        put("power_reasons", "brownout", "below_reserve", "at_reserve", "approaching_reserve", "above_reserve",
                "no_pack", "no_reading", "no_reserve", "no_capacity", "reading_unusable");
        put("power_fault_codes", "gauge_silent", "capacity_undeclared", "reserve_undeclared",
                "brownout_undeclarable", "charge_out_of_range", "thermal_sustained", "thermal_critical",
                "thermal_undeclared");
        put("thermal_pressures", "nominal", "throttling", "critical");
        put("shed_order", "telemetry", "ota", "blackbox_compression", "inference_rate", "non_essential_sensors");
        put("never_shed", "rhal", "kernel", "control_loop");
        put("reserve_terms", "declared_energy", "payload", "distance_to_safe_state");
        put("telemetry_window_faults", "unbounded", "inverted", "unmeasured_duration", "implausible_duration");
        put("upload_decisions", "done", "probe", "start", "resume", "restart", "wait_for_unmetered", "no_bearer",
                "refuse");
        put("upload_checks", "complete", "short", "long", "digest_mismatch");
        put("fetch_decisions", "have", "fetch", "resume", "discard_and_fetch", "wait_for_unmetered", "no_bearer",
                "refuse");
        put("bundle_checks", "match", "short", "long", "digest_mismatch");
        put("frame_refusal_codes", "unknown_stream", "unknown_slot", "stale_handle", "already_acquired",
                "double_release", "pool_exhausted");
    }

    /** Every set name, in declaration order, for the self-check to walk. */
    public static List<String> names() {
        return List.copyOf(SETS.keySet());
    }

    /**
     * The members of {@code input.set}, or JSON null for a name this edition does not hold.
     *
     * <p>Null and not an empty array, and the difference is the point: every one of the seventy-four sets
     * has at least one member, so an empty array could only ever mean <i>a set with nothing in it</i> —
     * which would read as an answer rather than as an absence. A vocabulary this edition has never heard
     * of is the second thing, and §54.2's rule about naming what could not be resolved applies to a set
     * name as much as to a path. The corpus never asks: all seventy-four of its cases name a set that
     * exists, and a case that did not would be a case this edition is right to refuse.
     */
    public static Value closedSetsCase(Value input) {
        Value wanted = Value.get(input, "set");
        if (wanted == null || wanted.kind() != Value.Kind.STRING) return Value.NULL;
        List<String> members = SETS.get(Value.asString(wanted, ""));
        if (members == null) return Value.NULL;
        Value out = Value.array();
        for (String member : members) out.push(Value.of(member));
        return out;
    }
}
