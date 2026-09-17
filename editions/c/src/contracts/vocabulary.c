#include "skynet_os/vocabulary.h"

#include <string.h>

/*
 * The sets, as they are declared in `reference/typescript` — which is where they were read from, one
 * exported constant per set, resolved through the `vocabulary()` calls in `tools/corpus-gen/generate.ts`
 * that name which constant each set is.
 *
 * `static const char *const[]` rather than a generated header: the whole point of the vocabulary corpus
 * is that a token added to one edition and not another is caught, and a table generated into every
 * edition from one source could not be caught by anything. This table is this edition's own statement of
 * what it believes the vocabulary to be, and the corpus is what says whether it is right.
 *
 * A compound literal in a static initialiser has static storage duration (C17 6.5.2.5p5), so `SET` costs
 * nothing at run time and there is no second name per set to keep in step with the first.
 */
#define SET(...) ((const char *const[]){__VA_ARGS__, NULL})

typedef struct {
    const char *name;
    const char *const *members;
} closed_set;

static const closed_set SETS[] = {
    {"package_rejection_codes", SET("unsigned", "signature_failed", "signature_untrusted",
        "signature_unverifiable", "digest_mismatch", "isolation_violation", "budget_exceeded",
        "capability_ungranted", "capability_denied", "capability_reserved", "over_request",
        "missing_dependency", "os_too_old", "transport_refused", "executes_on_load_untrusted",
        "environment_inert", "embodiment_undeclared", "capability_unknown")},
    {"signature_states", SET("verified", "failed", "untrusted", "absent", "unverifiable")},
    {"package_kinds", SET("policy", "skill", "driver", "service", "embodiment", "environment")},
    {"isolation_modes", SET("sandboxed", "native")},
    {"reserved_capabilities", SET("hri.warning")},
    {"capability_domains", SET("locomotion", "manipulation", "sensing", "control", "safety", "platform", "hri")},
    /* §85.1, in the order the section lists them. Membership is the whole rule, and not
       every token has two segments — `sensing.camera.rgb` has three. */
    {"capability_tokens", SET(
        "locomotion.walk", "locomotion.velocity", "locomotion.stairs",
        "locomotion.turn_in_place", "locomotion.stand", "locomotion.fly", "manipulation.arm",
        "manipulation.dual_arm", "manipulation.grasp", "manipulation.dexterous_hand",
        "manipulation.cartesian", "manipulation.force_control", "manipulation.mobile",
        "sensing.camera.rgb", "sensing.camera.depth", "sensing.camera.stereo", "sensing.lidar",
        "sensing.imu", "sensing.force_torque", "sensing.tactile", "sensing.joint_state",
        "sensing.odometry", "sensing.audio", "control.position", "control.velocity",
        "control.torque", "control.trajectory", "control.realtime", "control.motion",
        "safety.estop", "safety.collision_avoidance", "safety.workspace_limits",
        "platform.onboard_compute", "platform.ros2", "platform.teleop",
        "platform.data_recording", "hri.speech", "hri.warning")},
    {"token_classes", SET("canonical", "unknown", "vendor", "malformed")},
    {"transport_verdicts", SET("direct", "retargeted", "degraded", "refused")},
    {"transport_methods", SET("direct", "remap", "retarget", "synthesize", "drop", "blocked")},
    {"transport_warning_codes", SET("no_source_embodiment", "unverified_embodiment", "no_target_envelope",
        "dof_mismatch", "control_mode_unsupported", "rate_unsupported", "rate_below_watchdog", "scaled_down",
        "unscalable_channel", "retarget_without_solver", "unmapped_roles", "observation_missing",
        "observation_degraded", "latency_undeclared", "action_space_mismatch", "action_space_ambiguous",
        "motion_parameter_unused",
        /* §86, appended for the reason every other addition here is. */
        "joint_mode_conflict", "mixed_modes_unsupported")},
    {"action_kinds", SET("joint_position", "joint_velocity", "joint_torque", "cartesian", "base_velocity",
        "gripper", "discrete")},
    {"control_modes", SET("position", "velocity", "torque", "trajectory", "cartesian")},
    {"joint_types", SET("revolute", "prismatic", "continuous", "fixed")},
    {"base_kinds", SET("fixed", "wheeled", "tracked", "legged", "aerial", "rail")},
    {"embodiment_modalities", SET("rgb", "depth", "stereo", "point_cloud", "imu", "force_torque", "tactile",
        "audio", "joint_state")},
    {"canonical_chains", SET("left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
        "torso", "neck", "head", "base", "tail", "aux")},
    {"canonical_axes", SET("pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip")},
    {"canonical_motions", SET("move_joint", "move_linear", "move_circular", "grasp", "release", "home", "wait",
        "tool_change", "io_set")},
    {"motion_fault_codes", SET("unnamed", "duplicate_motion", "unknown_motion", "duplicate_parameter",
        "required_undeclared", "bound_invalid")},
    {"os_tiers", SET("guest", "host", "certified")},
    {"enforcement_modes", SET("advisory", "enforcing", "enforcing_redundant")},
    {"guards", SET("watchdog", "envelope", "rate_limit", "workspace", "self_collision", "balance", "proximity",
        "mode_gate")},
    {"fault_responses", SET("clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0")},
    {"scts_suites", SET("sim", "bench", "field")},
    {"scts_areas", SET("rhal", "embodiment", "safety", "package", "ota", "telemetry", "offline", "security",
        "power")},
    {"predicate_ops", SET("and", "or", "not", "eq", "neq", "in", "gte", "lte", "gt", "lt", "includes",
        "includes_all", "includes_any", "exists", "semver_satisfies")},
    {"predicate_outcomes", SET("satisfied", "unsatisfied", "unevaluable")},
    {"unevaluable_reasons", SET("malformed", "unknown_op", "missing_path", "missing_operand",
        "unresolved_path", "not_a_number", "not_a_version")},
    {"clause_outcomes", SET("satisfied", "unsatisfied", "unevaluable", "unenforceable")},
    {"clause_origins", SET("skill", "operator")},
    {"precondition_phases", SET("entry", "hold")},
    {"skill_outcomes", SET("success", "failure", "aborted", "unlabeled")},
    {"composition_faults", SET("cycle", "too_deep")},
    {"cell_faults", SET("unnamed", "fact_key_unusable", "fact_key_reserved", "volume_frame_missing",
        "volume_shape_invalid", "fixture_unnamed", "duplicate_fixture", "parking_channel_unnamed",
        "duplicate_parking_channel",
        /* §87.1, and all five apply only from the second robot. */
        "robot_unnamed", "duplicate_robot", "robot_unplaced", "robot_unbounded", "shared_volume")},
    {"safe_state_bases", SET("fixed", "mapped_worst_case")},
    {"safe_state_faults", SET("basis_undeclared", "basis_refused", "basis_unknown", "area_undeclared", "mapped_on_a_fixed_base", "figure_undeclared")},
    {"relay_routes", SET("lan", "relay")},
    {"console_roles", SET("anonymous", "observer", "operator", "admin")},
    {"console_actions", SET("stop_request", "read_status", "read_health", "start_package", "stop_package", "grant_capability", "revoke_capability", "seal_blackbox", "apply_update", "pair", "install_driver", "change_profile", "factory_reset")},
    {"session_states", SET("live", "expired", "refused")},
    {"session_refusals", SET("deadline_undeclared", "route_unknown", "role_unknown")},
    {"action_refusals", SET("role_insufficient", "presence_required", "session_expired", "session_refused")},
    {"motion_refusals", SET("no_lease", "estop", "not_active", "busy", "unknown_motion", "deadline_undeclared", "parameter_unknown", "parameter_missing", "parameter_not_finite", "parameter_out_of_bounds")},
    {"motion_ready_states", SET("running", "degraded")},
    {"motion_progress", SET("running", "overdue")},
    {"operating_states", SET("safe", "stopping", "post", "unhomed", "hold", "degraded", "running")},
    {"state_reasons", SET("rhal_lost", "safe_sticky", "post_failed", "stop_complete", "stopping_watchdog",
        "stopping_proximity", "stopping_balance", "stopping_operator", "stopping_estop", "post_incomplete",
        "homing_pending", "kernel_down", "guard_hold", "no_policy", "policy_not_ready", "guards_unavailable",
        "rhal_degraded", "nominal")},
    {"stop_causes", SET("watchdog", "proximity", "balance", "operator", "estop")},
    {"post_verdicts", SET("passed", "failed", "incomplete")},
    {"homing_states", SET("not_required", "pending", "complete")},
    {"rhal_states", SET("present", "degraded", "lost")},
    {"policy_states", SET("none", "held", "ready")},
    {"kernel_states", SET("live", "down")},
    {"skill_phases", SET("idle", "preconditions", "acquiring", "running", "verifying", "terminal")},
    {"lifecycle_reasons", SET("not_invoked", "invoked", "preconditions_pending", "preconditions_refused",
        "preconditions_met", "leases_held_by_root", "lease_pending", "lease_denied", "power_withheld",
        "leases_acquired", "timeout", "hold_lost", "guard_tripped", "robot_stopped", "running", "verifying",
        "verdict_pending", "judged", "terminal")},
    {"lease_states", SET("pending", "held", "denied")},
    {"os_components", SET("bootloader", "kernel_image", "rhal_abi", "rhal_driver", "safety_kernel", "envelope",
        "skynetd", "policy_runtime", "userspace")},
    {"certification_gated", SET("safety_kernel", "envelope")},
    {"bundle_faults", SET("unversioned", "undigested", "signature_failed", "signature_untrusted",
        "signature_absent", "signature_unverifiable", "incompatible", "compatibility_undeclared",
        "channel_mismatch", "certification_required", "bench_pass_stale", "bench_pass_failed", "already_running")},
    {"boot_decisions", SET("not_pending", "wait", "confirm", "roll_back")},
    {"code_faults", SET("empty", "looks_like_a_credential", "too_short", "too_long", "not_in_alphabet")},
    {"device_scopes", SET("device:calibration", "device:checkin", "device:conformance", "device:desired",
        "device:episodes", "device:evaluations", "device:incidents", "device:releases")},
    {"key_bindings", SET("hardware", "software")},
    {"identity_findings", SET("not_paired", "namespace_missing", "not_a_device_credential",
        "scope_unrecognised", "scope_undeclared", "key_binding_unknown", "key_not_hardware_bound",
        "key_not_attested", "no_birth_certificate")},
    {"measure_states", SET("known", "nothing", "unmeasured")},
    {"birth_findings", SET("unattested", "device_key_mismatch", "serial_mismatch", "descriptor_mismatch",
        "hardware_revision_mismatch", "ownership_asserted", "unobservable", "incomplete")},
    {"birth_required_fields", SET("device_key", "model", "serial", "descriptor_digest", "hardware_revision",
        "manufactured")},
    {"birth_ownership_fields", SET("namespace", "owner", "operator", "customer")},
    {"provenance_classes", SET("field", "simulated", "unattested")},
    {"provenance_reason_codes", SET("device_signature", "device_unknown", "simulator_declared",
        "birth_unattested", "image_unattested", "seeds_not_supplied", "benchmark_mismatch", "not_replayable")},
    {"checkin_claims", SET("applied", "converging", "deferred", "inconsistent")},
    {"reconcile_stages", SET("os_image", "rhal_driver", "package", "profile", "policy", "lease")},
    {"readiness_states", SET("ready", "stalled", "not_running", "unknown")},
    {"health_fields", SET("generation_lag", "guard_trip_rate", "storage_headroom", "battery", "temperature_c")},
    {"offer_faults", SET("undecodable", "generation_reused", "generation_went_backwards", "empties_the_robot")},
    {"telemetry_counters", SET("cycles", "guard_trips", "clamps", "interventions", "estops",
        "package_restarts", "queue_dropped", "reconcile_passes")},
    {"power_states", SET("normal", "low", "reserve", "critical", "unmetered")},
    {"power_reasons", SET("brownout", "below_reserve", "at_reserve", "approaching_reserve", "above_reserve",
        "no_pack", "no_reading", "no_reserve", "no_capacity", "reading_unusable")},
    {"power_fault_codes", SET("gauge_silent", "capacity_undeclared", "reserve_undeclared",
        "brownout_undeclarable", "charge_out_of_range", "thermal_sustained", "thermal_critical",
        "thermal_undeclared")},
    {"thermal_pressures", SET("nominal", "throttling", "critical")},
    {"shed_order", SET("telemetry", "ota", "blackbox_compression", "inference_rate", "non_essential_sensors")},
    {"never_shed", SET("rhal", "kernel", "control_loop")},
    {"reserve_terms", SET("declared_energy", "payload", "distance_to_safe_state")},
    {"telemetry_window_faults", SET("unbounded", "inverted", "unmeasured_duration", "implausible_duration")},
    {"upload_decisions", SET("done", "probe", "start", "resume", "restart", "wait_for_unmetered", "no_bearer",
        "refuse")},
    {"upload_checks", SET("complete", "short", "long", "digest_mismatch")},
    {"fetch_decisions", SET("have", "fetch", "resume", "discard_and_fetch", "wait_for_unmetered", "no_bearer",
        "refuse")},
    {"bundle_checks", SET("match", "short", "long", "digest_mismatch")},
    {"frame_refusal_codes", SET("unknown_stream", "unknown_slot", "stale_handle", "already_acquired",
        "double_release", "pool_exhausted")},
};

#define SK_SET_COUNT (sizeof SETS / sizeof SETS[0])

const char *const *sk_vocabulary_set_names(size_t *count)
{
    /* One entry per set plus the terminator, built once on the first call. `static` rather than an arena
     * allocation because the self-check asks for it before any case has an arena, and because the answer
     * cannot depend on which arena asked. */
    static const char *names[SK_SET_COUNT + 1];
    static bool built = false;
    if (!built) {
        for (size_t at = 0; at < SK_SET_COUNT; at += 1) {
            names[at] = SETS[at].name;
        }
        names[SK_SET_COUNT] = NULL;
        built = true;
    }
    *count = SK_SET_COUNT;
    return names;
}

/*
 * The members of `input.set`, or JSON null for a name this edition does not hold.
 *
 * Null and not an empty array, and the difference is the point: every one of the seventy-four sets has at
 * least one member, so an empty array could only ever mean *a set with nothing in it* — which would read
 * as an answer rather than as an absence. A vocabulary this edition has never heard of is the second
 * thing, and §54.2's rule about naming what could not be resolved applies to a set name as much as to a
 * path. The corpus never asks: all seventy-four of its cases name a set that exists, and a case that did
 * not would be a case this edition is right to refuse.
 */
const sk_value *sk_vocabulary_closed_sets_case(sk_arena *arena, const sk_value *input,
                                               const char **error)
{
    (void) error;
    const sk_value *wanted = sk_get(input, "set");
    if (sk_kind_of(wanted) != SK_STRING) {
        return sk_null();
    }
    for (size_t at = 0; at < SK_SET_COUNT; at += 1) {
        if (!sk_string_is(wanted, SETS[at].name)) {
            continue;
        }
        sk_value *out = sk_array(arena);
        for (const char *const *member = SETS[at].members; *member != NULL; member += 1) {
            sk_push(arena, out, sk_string(arena, *member));
        }
        return out;
    }
    return sk_null();
}
