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
// **The members' order is frozen output; the sets' order is not.** Each set's members are a `string[]`,
// which promises its order, and that is the order the corpus compares bytes against. The order of the
// *sets themselves* affects nothing an edition emits — only the order the self-check walks them in and
// reports a failure in — so a `Dictionary`, which promises no enumeration order, is the right container
// for the lookup. Saying so is worth a sentence, because the opposite mistake (relying on a dictionary's
// insertion order for something that is output) is the class of bug that works in every test.
//
// The table is this edition's own statement of what it believes the vocabulary to be, written out rather
// than generated into every edition from one source — a token added to one edition and not another is the
// divergence the corpus exists to catch, and it cannot catch one that a generator put in all of them.

using System;
using System.Collections.Generic;

namespace Skynet.Os.Contracts;

internal static class Vocabulary
{
    private static string[] Set(params string[] members) => members;

    /// <summary>
    /// The sets, as they are declared in <c>reference/typescript</c> — one exported constant per set,
    /// resolved through the <c>vocabulary()</c> calls in <c>tools/corpus-gen/generate.ts</c> that name
    /// which constant each set is.
    /// </summary>
    /// <remarks>
    /// A collection initialiser with a duplicate key throws <c>ArgumentException</c> at type
    /// initialisation — §6.1's ladder, where a violation that cannot be represented beats a test that
    /// finds it. A duplicate would otherwise replace the earlier set in place and lose it, and the corpus
    /// would pass because one of the two is the answer it measures.
    /// </remarks>
    private static readonly Dictionary<string, string[]> Sets = new(StringComparer.Ordinal)
    {
        ["package_rejection_codes"] = Set("unsigned", "signature_failed", "signature_untrusted",
            "signature_unverifiable", "digest_mismatch", "isolation_violation", "budget_exceeded",
            "capability_ungranted", "capability_denied", "capability_reserved", "over_request",
            "missing_dependency", "os_too_old", "transport_refused", "executes_on_load_untrusted",
            "environment_inert", "embodiment_undeclared", "capability_unknown"),
        ["signature_states"] = Set("verified", "failed", "untrusted", "absent", "unverifiable"),
        ["package_kinds"] = Set("policy", "skill", "driver", "service", "embodiment", "environment"),
        ["isolation_modes"] = Set("sandboxed", "native"),
        ["reserved_capabilities"] = Set("hri.warning"),
        ["capability_domains"] = Set("locomotion", "manipulation", "sensing", "control", "safety", "platform", "hri"),
        // §85.1, in the order the section lists them. Membership is the whole rule, and not every
        // token has two segments — `sensing.camera.rgb` has three.
        ["capability_tokens"] = Set(
            "locomotion.walk", "locomotion.velocity", "locomotion.stairs", "locomotion.turn_in_place",
            "locomotion.stand", "locomotion.fly", "manipulation.arm", "manipulation.dual_arm",
            "manipulation.grasp", "manipulation.dexterous_hand", "manipulation.cartesian",
            "manipulation.force_control", "manipulation.mobile", "sensing.camera.rgb", "sensing.camera.depth",
            "sensing.camera.stereo", "sensing.lidar", "sensing.imu", "sensing.force_torque",
            "sensing.tactile", "sensing.joint_state", "sensing.odometry", "sensing.audio", "control.position",
            "control.velocity", "control.torque", "control.trajectory", "control.realtime", "control.motion",
            "safety.estop", "safety.collision_avoidance", "safety.workspace_limits",
            "platform.onboard_compute", "platform.ros2", "platform.teleop", "platform.data_recording",
            "hri.speech", "hri.warning"),
        ["token_classes"] = Set("canonical", "unknown", "vendor", "malformed"),
        ["transport_verdicts"] = Set("direct", "retargeted", "degraded", "refused"),
        ["transport_methods"] = Set("direct", "remap", "retarget", "synthesize", "drop", "blocked"),
        ["transport_warning_codes"] = Set("no_source_embodiment", "unverified_embodiment", "no_target_envelope",
            "dof_mismatch", "control_mode_unsupported", "rate_unsupported", "rate_below_watchdog", "scaled_down",
            "unscalable_channel", "retarget_without_solver", "unmapped_roles", "observation_missing",
            "observation_degraded", "latency_undeclared", "action_space_mismatch", "action_space_ambiguous",
            "motion_parameter_unused"),
        ["action_kinds"] = Set("joint_position", "joint_velocity", "joint_torque", "cartesian", "base_velocity",
            "gripper", "discrete"),
        ["control_modes"] = Set("position", "velocity", "torque", "trajectory", "cartesian"),
        ["joint_types"] = Set("revolute", "prismatic", "continuous", "fixed"),
        ["base_kinds"] = Set("fixed", "wheeled", "tracked", "legged", "aerial", "rail"),
        ["embodiment_modalities"] = Set("rgb", "depth", "stereo", "point_cloud", "imu", "force_torque", "tactile",
            "audio", "joint_state"),
        ["canonical_chains"] = Set("left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
            "torso", "neck", "head", "base", "tail", "aux"),
        ["canonical_axes"] = Set("pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip"),
        ["canonical_motions"] = Set("move_joint", "move_linear", "move_circular", "grasp", "release", "home",
            "wait", "tool_change", "io_set"),
        ["motion_fault_codes"] = Set("unnamed", "duplicate_motion", "unknown_motion", "duplicate_parameter",
            "required_undeclared", "bound_invalid"),
        ["os_tiers"] = Set("guest", "host", "certified"),
        ["enforcement_modes"] = Set("advisory", "enforcing", "enforcing_redundant"),
        ["guards"] = Set("watchdog", "envelope", "rate_limit", "workspace", "self_collision", "balance",
            "proximity", "mode_gate"),
        ["fault_responses"] = Set("clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0"),
        ["scts_suites"] = Set("sim", "bench", "field"),
        ["scts_areas"] = Set("rhal", "embodiment", "safety", "package", "ota", "telemetry", "offline", "security",
            "power"),
        ["predicate_ops"] = Set("and", "or", "not", "eq", "neq", "in", "gte", "lte", "gt", "lt", "includes",
            "includes_all", "includes_any", "exists", "semver_satisfies"),
        ["predicate_outcomes"] = Set("satisfied", "unsatisfied", "unevaluable"),
        ["unevaluable_reasons"] = Set("malformed", "unknown_op", "missing_path", "missing_operand",
            "unresolved_path", "not_a_number", "not_a_version"),
        ["clause_outcomes"] = Set("satisfied", "unsatisfied", "unevaluable", "unenforceable"),
        ["clause_origins"] = Set("skill", "operator"),
        ["precondition_phases"] = Set("entry", "hold"),
        ["skill_outcomes"] = Set("success", "failure", "aborted", "unlabeled"),
        ["composition_faults"] = Set("cycle", "too_deep"),
        ["cell_faults"] = Set("unnamed", "fact_key_unusable", "fact_key_reserved", "volume_frame_missing",
            "volume_shape_invalid", "fixture_unnamed", "duplicate_fixture", "parking_channel_unnamed",
            "duplicate_parking_channel"),
        ["operating_states"] = Set("safe", "stopping", "post", "unhomed", "hold", "degraded", "running"),
        ["state_reasons"] = Set("rhal_lost", "safe_sticky", "post_failed", "stop_complete", "stopping_watchdog",
            "stopping_proximity", "stopping_balance", "stopping_operator", "stopping_estop", "post_incomplete",
            "homing_pending", "kernel_down", "guard_hold", "no_policy", "policy_not_ready", "guards_unavailable",
            "rhal_degraded", "nominal"),
        ["stop_causes"] = Set("watchdog", "proximity", "balance", "operator", "estop"),
        ["post_verdicts"] = Set("passed", "failed", "incomplete"),
        ["homing_states"] = Set("not_required", "pending", "complete"),
        ["rhal_states"] = Set("present", "degraded", "lost"),
        ["policy_states"] = Set("none", "held", "ready"),
        ["kernel_states"] = Set("live", "down"),
        ["skill_phases"] = Set("idle", "preconditions", "acquiring", "running", "verifying", "terminal"),
        ["lifecycle_reasons"] = Set("not_invoked", "invoked", "preconditions_pending", "preconditions_refused",
            "preconditions_met", "leases_held_by_root", "lease_pending", "lease_denied", "power_withheld",
            "leases_acquired", "timeout", "hold_lost", "guard_tripped", "robot_stopped", "running", "verifying",
            "verdict_pending", "judged", "terminal"),
        ["lease_states"] = Set("pending", "held", "denied"),
        ["os_components"] = Set("bootloader", "kernel_image", "rhal_abi", "rhal_driver", "safety_kernel",
            "envelope", "skynetd", "policy_runtime", "userspace"),
        ["certification_gated"] = Set("safety_kernel", "envelope"),
        ["bundle_faults"] = Set("unversioned", "undigested", "signature_failed", "signature_untrusted",
            "signature_absent", "signature_unverifiable", "incompatible", "compatibility_undeclared",
            "channel_mismatch", "certification_required", "bench_pass_stale", "bench_pass_failed", "already_running"),
        ["boot_decisions"] = Set("not_pending", "wait", "confirm", "roll_back"),
        ["code_faults"] = Set("empty", "looks_like_a_credential", "too_short", "too_long", "not_in_alphabet"),
        ["device_scopes"] = Set("device:calibration", "device:checkin", "device:conformance", "device:desired",
            "device:episodes", "device:evaluations", "device:incidents", "device:releases"),
        ["key_bindings"] = Set("hardware", "software"),
        ["identity_findings"] = Set("not_paired", "namespace_missing", "not_a_device_credential",
            "scope_unrecognised", "scope_undeclared", "key_binding_unknown", "key_not_hardware_bound",
            "key_not_attested", "no_birth_certificate"),
        ["measure_states"] = Set("known", "nothing", "unmeasured"),
        ["birth_findings"] = Set("unattested", "device_key_mismatch", "serial_mismatch", "descriptor_mismatch",
            "hardware_revision_mismatch", "ownership_asserted", "unobservable", "incomplete"),
        ["birth_required_fields"] = Set("device_key", "model", "serial", "descriptor_digest", "hardware_revision",
            "manufactured"),
        ["birth_ownership_fields"] = Set("namespace", "owner", "operator", "customer"),
        ["provenance_classes"] = Set("field", "simulated", "unattested"),
        ["provenance_reason_codes"] = Set("device_signature", "device_unknown", "simulator_declared",
            "birth_unattested", "image_unattested", "seeds_not_supplied", "benchmark_mismatch", "not_replayable"),
        ["checkin_claims"] = Set("applied", "converging", "deferred", "inconsistent"),
        ["reconcile_stages"] = Set("os_image", "rhal_driver", "package", "profile", "policy", "lease"),
        ["readiness_states"] = Set("ready", "stalled", "not_running", "unknown"),
        ["health_fields"] = Set("generation_lag", "guard_trip_rate", "storage_headroom", "battery", "temperature_c"),
        ["offer_faults"] = Set("undecodable", "generation_reused", "generation_went_backwards", "empties_the_robot"),
        ["telemetry_counters"] = Set("cycles", "guard_trips", "clamps", "interventions", "estops",
            "package_restarts", "queue_dropped", "reconcile_passes"),
        ["power_states"] = Set("normal", "low", "reserve", "critical", "unmetered"),
        ["power_reasons"] = Set("brownout", "below_reserve", "at_reserve", "approaching_reserve", "above_reserve",
            "no_pack", "no_reading", "no_reserve", "no_capacity", "reading_unusable"),
        ["power_fault_codes"] = Set("gauge_silent", "capacity_undeclared", "reserve_undeclared",
            "brownout_undeclarable", "charge_out_of_range", "thermal_sustained", "thermal_critical",
            "thermal_undeclared"),
        ["thermal_pressures"] = Set("nominal", "throttling", "critical"),
        ["shed_order"] = Set("telemetry", "ota", "blackbox_compression", "inference_rate", "non_essential_sensors"),
        ["never_shed"] = Set("rhal", "kernel", "control_loop"),
        ["reserve_terms"] = Set("declared_energy", "payload", "distance_to_safe_state"),
        ["telemetry_window_faults"] = Set("unbounded", "inverted", "unmeasured_duration", "implausible_duration"),
        ["upload_decisions"] = Set("done", "probe", "start", "resume", "restart", "wait_for_unmetered", "no_bearer",
            "refuse"),
        ["upload_checks"] = Set("complete", "short", "long", "digest_mismatch"),
        ["fetch_decisions"] = Set("have", "fetch", "resume", "discard_and_fetch", "wait_for_unmetered", "no_bearer",
            "refuse"),
        ["bundle_checks"] = Set("match", "short", "long", "digest_mismatch"),
        ["frame_refusal_codes"] = Set("unknown_stream", "unknown_slot", "stale_handle", "already_acquired",
            "double_release", "pool_exhausted"),
    };

    /// <summary>
    /// Every set name, for the self-check to walk. In whatever order the dictionary enumerates, which is
    /// deliberately not relied on: see the note at the top of this file — the sets' order is not output.
    /// </summary>
    internal static IEnumerable<string> Names => Sets.Keys;

    /// <summary>How many sets this edition holds. What the self-check pins against the corpus's seventy-four.</summary>
    internal static int Count => Sets.Count;

    /// <summary>
    /// The members of <c>input.set</c>, or JSON null for a name this edition does not hold.
    /// <para>
    /// Null and not an empty array, and the difference is the point: every one of the seventy-four sets
    /// has at least one member, so an empty array could only ever mean <i>a set with nothing in it</i> —
    /// which would read as an answer rather than as an absence. A vocabulary this edition has never heard
    /// of is the second thing, and §54.2's rule about naming what could not be resolved applies to a set
    /// name as much as to a path. The corpus never asks: all seventy-four of its cases name a set that
    /// exists, and a case that did not would be a case this edition is right to refuse.
    /// </para>
    /// </summary>
    internal static Value ClosedSetsCase(Value? input)
    {
        Value? wanted = Value.Get(input, "set");
        if (wanted is null || wanted.ValueKind != Kind.String)
        {
            return Value.Null;
        }
        if (!Sets.TryGetValue(Value.AsString(wanted, string.Empty), out string[]? members))
        {
            return Value.Null;
        }
        Value out_ = Value.Array();
        foreach (string member in members)
        {
            out_.Push(Value.Of(member));
        }
        return out_;
    }
}
