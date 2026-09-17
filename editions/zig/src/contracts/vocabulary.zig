//! The closed string sets — `corpus/vocabulary/closed-sets.json`, and the only contract in this edition
//! with no module of its own in `reference/typescript`.
//!
//! Every other corpus holds an edition to a *computation*: give it this input, emit these bytes. This one
//! holds it to a **vocabulary**, and the generator's own comment says why it exists: `capability_reserved`
//! was added to the Rust `PackageRejectionCode` and not to the reference's, and nothing could see it — the
//! mirror pins guard the reference, and every corpus covers the cases it holds, none of which had ever
//! mentioned a token invented ten minutes earlier. A missing member is now a corpus failure on the first
//! run, with the set named.
//!
//! Seventy-four sets, and **order is part of the agreement** for several of them: `os_tiers` and
//! `scts_areas` are ranked, `operating_states` and `state_reasons` run in the order `evaluate` descends
//! through its rules, and `skill_phases` runs in diagram order so that "never moves backwards through this
//! list" is a property somebody can assert. An edition that sorted any of those alphabetically would pass
//! every other corpus and would have lost the property.
//!
//! The table is this edition's own statement of what it believes the vocabulary to be, written out rather
//! than generated into every edition from one source — a token added to one edition and not another is the
//! divergence the corpus exists to catch, and it cannot catch one that a generator put in all of them.

const std = @import("std");

const value = @import("../value.zig");

const Allocator = std.mem.Allocator;
const Value = value.Value;

const ClosedSet = struct {
    name: []const u8,
    members: []const []const u8,
};

/// The sets, as they are declared in `reference/typescript` — one exported constant per set, resolved
/// through the `vocabulary()` calls in `tools/corpus-gen/generate.ts` that name which constant each is.
///
/// One member per line, which is `zig fmt`'s own choice once a list carries a trailing comma — and the
/// right choice here for a reason beyond the formatter: the divergence this corpus exists to catch is a
/// token added to one edition and not another, and one member per line makes that exactly one line of
/// diff. A wrapped list would hide it inside a reflowed paragraph.
const sets = [_]ClosedSet{
    .{
        .name = "package_rejection_codes",
        .members = &.{
            "unsigned",
            "signature_failed",
            "signature_untrusted",
            "signature_unverifiable",
            "digest_mismatch",
            "isolation_violation",
            "budget_exceeded",
            "capability_ungranted",
            "capability_denied",
            "capability_reserved",
            "over_request",
            "missing_dependency",
            "os_too_old",
            "transport_refused",
            "executes_on_load_untrusted",
            "environment_inert",
            "embodiment_undeclared",
            "capability_unknown",
        },
    },
    .{
        .name = "signature_states",
        .members = &.{
            "verified",
            "failed",
            "untrusted",
            "absent",
            "unverifiable",
        },
    },
    .{
        .name = "package_kinds",
        .members = &.{
            "policy",
            "skill",
            "driver",
            "service",
            "embodiment",
            "environment",
        },
    },
    .{
        .name = "isolation_modes",
        .members = &.{
            "sandboxed",
            "native",
        },
    },
    .{
        .name = "reserved_capabilities",
        .members = &.{
            "hri.warning",
        },
    },
    .{
        .name = "capability_domains",
        .members = &.{
            "locomotion",
            "manipulation",
            "sensing",
            "control",
            "safety",
            "platform",
            "hri",
        },
    },
    // §85.1, in the order the section lists them. Membership is the whole rule, and not every
    // token has two segments — `sensing.camera.rgb` has three.
    .{
        .name = "capability_tokens",
        .members = &.{
            "locomotion.walk",
            "locomotion.velocity",
            "locomotion.stairs",
            "locomotion.turn_in_place",
            "locomotion.stand",
            "locomotion.fly",
            "manipulation.arm",
            "manipulation.dual_arm",
            "manipulation.grasp",
            "manipulation.dexterous_hand",
            "manipulation.cartesian",
            "manipulation.force_control",
            "manipulation.mobile",
            "sensing.camera.rgb",
            "sensing.camera.depth",
            "sensing.camera.stereo",
            "sensing.lidar",
            "sensing.imu",
            "sensing.force_torque",
            "sensing.tactile",
            "sensing.joint_state",
            "sensing.odometry",
            "sensing.audio",
            "control.position",
            "control.velocity",
            "control.torque",
            "control.trajectory",
            "control.realtime",
            "control.motion",
            "safety.estop",
            "safety.collision_avoidance",
            "safety.workspace_limits",
            "platform.onboard_compute",
            "platform.ros2",
            "platform.teleop",
            "platform.data_recording",
            "hri.speech",
            "hri.warning",
        },
    },
    .{
        .name = "token_classes",
        .members = &.{
            "canonical",
            "unknown",
            "vendor",
            "malformed",
        },
    },
    .{
        .name = "transport_verdicts",
        .members = &.{
            "direct",
            "retargeted",
            "degraded",
            "refused",
        },
    },
    .{
        .name = "transport_methods",
        .members = &.{
            "direct",
            "remap",
            "retarget",
            "synthesize",
            "drop",
            "blocked",
        },
    },
    .{
        .name = "transport_warning_codes",
        .members = &.{
            "no_source_embodiment",
            "unverified_embodiment",
            "no_target_envelope",
            "dof_mismatch",
            "control_mode_unsupported",
            "rate_unsupported",
            "rate_below_watchdog",
            "scaled_down",
            "unscalable_channel",
            "retarget_without_solver",
            "unmapped_roles",
            "observation_missing",
            "observation_degraded",
            "latency_undeclared",
            "action_space_mismatch",
            "action_space_ambiguous",
            "motion_parameter_unused",
        },
    },
    .{
        .name = "action_kinds",
        .members = &.{
            "joint_position",
            "joint_velocity",
            "joint_torque",
            "cartesian",
            "base_velocity",
            "gripper",
            "discrete",
        },
    },
    .{
        .name = "control_modes",
        .members = &.{
            "position",
            "velocity",
            "torque",
            "trajectory",
            "cartesian",
        },
    },
    .{
        .name = "joint_types",
        .members = &.{
            "revolute",
            "prismatic",
            "continuous",
            "fixed",
        },
    },
    .{
        .name = "base_kinds",
        .members = &.{
            "fixed",
            "wheeled",
            "tracked",
            "legged",
            "aerial",
            "rail",
        },
    },
    .{
        .name = "embodiment_modalities",
        .members = &.{
            "rgb",
            "depth",
            "stereo",
            "point_cloud",
            "imu",
            "force_torque",
            "tactile",
            "audio",
            "joint_state",
        },
    },
    .{
        .name = "canonical_chains",
        .members = &.{
            "left_arm",
            "right_arm",
            "left_leg",
            "right_leg",
            "left_hand",
            "right_hand",
            "torso",
            "neck",
            "head",
            "base",
            "tail",
            "aux",
        },
    },
    .{
        .name = "canonical_axes",
        .members = &.{
            "pitch",
            "yaw",
            "roll",
            "slide",
            "flex",
            "abduct",
            "rotate",
            "grip",
        },
    },
    .{
        .name = "canonical_motions",
        .members = &.{
            "move_joint",
            "move_linear",
            "move_circular",
            "grasp",
            "release",
            "home",
            "wait",
            "tool_change",
            "io_set",
        },
    },
    .{
        .name = "motion_fault_codes",
        .members = &.{
            "unnamed",
            "duplicate_motion",
            "unknown_motion",
            "duplicate_parameter",
            "required_undeclared",
            "bound_invalid",
        },
    },
    .{
        .name = "os_tiers",
        .members = &.{
            "guest",
            "host",
            "certified",
        },
    },
    .{
        .name = "enforcement_modes",
        .members = &.{
            "advisory",
            "enforcing",
            "enforcing_redundant",
        },
    },
    .{
        .name = "guards",
        .members = &.{
            "watchdog",
            "envelope",
            "rate_limit",
            "workspace",
            "self_collision",
            "balance",
            "proximity",
            "mode_gate",
        },
    },
    .{
        .name = "fault_responses",
        .members = &.{
            "clamp",
            "hold",
            "stop_cat2",
            "stop_cat1",
            "stop_cat0",
        },
    },
    .{
        .name = "scts_suites",
        .members = &.{
            "sim",
            "bench",
            "field",
        },
    },
    .{
        .name = "scts_areas",
        .members = &.{
            "rhal",
            "embodiment",
            "safety",
            "package",
            "ota",
            "telemetry",
            "offline",
            "security",
            "power",
        },
    },
    .{
        .name = "predicate_ops",
        .members = &.{
            "and",
            "or",
            "not",
            "eq",
            "neq",
            "in",
            "gte",
            "lte",
            "gt",
            "lt",
            "includes",
            "includes_all",
            "includes_any",
            "exists",
            "semver_satisfies",
        },
    },
    .{
        .name = "predicate_outcomes",
        .members = &.{
            "satisfied",
            "unsatisfied",
            "unevaluable",
        },
    },
    .{
        .name = "unevaluable_reasons",
        .members = &.{
            "malformed",
            "unknown_op",
            "missing_path",
            "missing_operand",
            "unresolved_path",
            "not_a_number",
            "not_a_version",
        },
    },
    .{
        .name = "clause_outcomes",
        .members = &.{
            "satisfied",
            "unsatisfied",
            "unevaluable",
            "unenforceable",
        },
    },
    .{
        .name = "clause_origins",
        .members = &.{
            "skill",
            "operator",
        },
    },
    .{
        .name = "precondition_phases",
        .members = &.{
            "entry",
            "hold",
        },
    },
    .{
        .name = "skill_outcomes",
        .members = &.{
            "success",
            "failure",
            "aborted",
            "unlabeled",
        },
    },
    .{
        .name = "composition_faults",
        .members = &.{
            "cycle",
            "too_deep",
        },
    },
    .{
        .name = "cell_faults",
        .members = &.{
            "unnamed",
            "fact_key_unusable",
            "fact_key_reserved",
            "volume_frame_missing",
            "volume_shape_invalid",
            "fixture_unnamed",
            "duplicate_fixture",
            "parking_channel_unnamed",
            "duplicate_parking_channel",
        },
    },
    .{
        .name = "operating_states",
        .members = &.{
            "safe",
            "stopping",
            "post",
            "unhomed",
            "hold",
            "degraded",
            "running",
        },
    },
    .{
        .name = "state_reasons",
        .members = &.{
            "rhal_lost",
            "safe_sticky",
            "post_failed",
            "stop_complete",
            "stopping_watchdog",
            "stopping_proximity",
            "stopping_balance",
            "stopping_operator",
            "stopping_estop",
            "post_incomplete",
            "homing_pending",
            "kernel_down",
            "guard_hold",
            "no_policy",
            "policy_not_ready",
            "guards_unavailable",
            "rhal_degraded",
            "nominal",
        },
    },
    .{
        .name = "stop_causes",
        .members = &.{
            "watchdog",
            "proximity",
            "balance",
            "operator",
            "estop",
        },
    },
    .{
        .name = "post_verdicts",
        .members = &.{
            "passed",
            "failed",
            "incomplete",
        },
    },
    .{
        .name = "homing_states",
        .members = &.{
            "not_required",
            "pending",
            "complete",
        },
    },
    .{
        .name = "rhal_states",
        .members = &.{
            "present",
            "degraded",
            "lost",
        },
    },
    .{
        .name = "policy_states",
        .members = &.{
            "none",
            "held",
            "ready",
        },
    },
    .{
        .name = "kernel_states",
        .members = &.{
            "live",
            "down",
        },
    },
    .{
        .name = "skill_phases",
        .members = &.{
            "idle",
            "preconditions",
            "acquiring",
            "running",
            "verifying",
            "terminal",
        },
    },
    .{
        .name = "lifecycle_reasons",
        .members = &.{
            "not_invoked",
            "invoked",
            "preconditions_pending",
            "preconditions_refused",
            "preconditions_met",
            "leases_held_by_root",
            "lease_pending",
            "lease_denied",
            "power_withheld",
            "leases_acquired",
            "timeout",
            "hold_lost",
            "guard_tripped",
            "robot_stopped",
            "running",
            "verifying",
            "verdict_pending",
            "judged",
            "terminal",
        },
    },
    .{
        .name = "lease_states",
        .members = &.{
            "pending",
            "held",
            "denied",
        },
    },
    .{
        .name = "os_components",
        .members = &.{
            "bootloader",
            "kernel_image",
            "rhal_abi",
            "rhal_driver",
            "safety_kernel",
            "envelope",
            "skynetd",
            "policy_runtime",
            "userspace",
        },
    },
    .{
        .name = "certification_gated",
        .members = &.{
            "safety_kernel",
            "envelope",
        },
    },
    .{
        .name = "bundle_faults",
        .members = &.{
            "unversioned",
            "undigested",
            "signature_failed",
            "signature_untrusted",
            "signature_absent",
            "signature_unverifiable",
            "incompatible",
            "compatibility_undeclared",
            "channel_mismatch",
            "certification_required",
            "bench_pass_stale",
            "bench_pass_failed",
            "already_running",
        },
    },
    .{
        .name = "boot_decisions",
        .members = &.{
            "not_pending",
            "wait",
            "confirm",
            "roll_back",
        },
    },
    .{
        .name = "code_faults",
        .members = &.{
            "empty",
            "looks_like_a_credential",
            "too_short",
            "too_long",
            "not_in_alphabet",
        },
    },
    .{
        .name = "device_scopes",
        .members = &.{
            "device:calibration",
            "device:checkin",
            "device:conformance",
            "device:desired",
            "device:episodes",
            "device:evaluations",
            "device:incidents",
            "device:releases",
        },
    },
    .{
        .name = "key_bindings",
        .members = &.{
            "hardware",
            "software",
        },
    },
    .{
        .name = "identity_findings",
        .members = &.{
            "not_paired",
            "namespace_missing",
            "not_a_device_credential",
            "scope_unrecognised",
            "scope_undeclared",
            "key_binding_unknown",
            "key_not_hardware_bound",
            "key_not_attested",
            "no_birth_certificate",
        },
    },
    .{
        .name = "measure_states",
        .members = &.{
            "known",
            "nothing",
            "unmeasured",
        },
    },
    .{
        .name = "birth_findings",
        .members = &.{
            "unattested",
            "device_key_mismatch",
            "serial_mismatch",
            "descriptor_mismatch",
            "hardware_revision_mismatch",
            "ownership_asserted",
            "unobservable",
            "incomplete",
        },
    },
    .{
        .name = "birth_required_fields",
        .members = &.{
            "device_key",
            "model",
            "serial",
            "descriptor_digest",
            "hardware_revision",
            "manufactured",
        },
    },
    .{
        .name = "birth_ownership_fields",
        .members = &.{
            "namespace",
            "owner",
            "operator",
            "customer",
        },
    },
    .{
        .name = "provenance_classes",
        .members = &.{
            "field",
            "simulated",
            "unattested",
        },
    },
    .{
        .name = "provenance_reason_codes",
        .members = &.{
            "device_signature",
            "device_unknown",
            "simulator_declared",
            "birth_unattested",
            "image_unattested",
            "seeds_not_supplied",
            "benchmark_mismatch",
            "not_replayable",
        },
    },
    .{
        .name = "checkin_claims",
        .members = &.{
            "applied",
            "converging",
            "deferred",
            "inconsistent",
        },
    },
    .{
        .name = "reconcile_stages",
        .members = &.{
            "os_image",
            "rhal_driver",
            "package",
            "profile",
            "policy",
            "lease",
        },
    },
    .{
        .name = "readiness_states",
        .members = &.{
            "ready",
            "stalled",
            "not_running",
            "unknown",
        },
    },
    .{
        .name = "health_fields",
        .members = &.{
            "generation_lag",
            "guard_trip_rate",
            "storage_headroom",
            "battery",
            "temperature_c",
        },
    },
    .{
        .name = "offer_faults",
        .members = &.{
            "undecodable",
            "generation_reused",
            "generation_went_backwards",
            "empties_the_robot",
        },
    },
    .{
        .name = "telemetry_counters",
        .members = &.{
            "cycles",
            "guard_trips",
            "clamps",
            "interventions",
            "estops",
            "package_restarts",
            "queue_dropped",
            "reconcile_passes",
        },
    },
    .{
        .name = "power_states",
        .members = &.{
            "normal",
            "low",
            "reserve",
            "critical",
            "unmetered",
        },
    },
    .{
        .name = "power_reasons",
        .members = &.{
            "brownout",
            "below_reserve",
            "at_reserve",
            "approaching_reserve",
            "above_reserve",
            "no_pack",
            "no_reading",
            "no_reserve",
            "no_capacity",
            "reading_unusable",
        },
    },
    .{
        .name = "power_fault_codes",
        .members = &.{
            "gauge_silent",
            "capacity_undeclared",
            "reserve_undeclared",
            "brownout_undeclarable",
            "charge_out_of_range",
            "thermal_sustained",
            "thermal_critical",
            "thermal_undeclared",
        },
    },
    .{
        .name = "thermal_pressures",
        .members = &.{
            "nominal",
            "throttling",
            "critical",
        },
    },
    .{
        .name = "shed_order",
        .members = &.{
            "telemetry",
            "ota",
            "blackbox_compression",
            "inference_rate",
            "non_essential_sensors",
        },
    },
    .{
        .name = "never_shed",
        .members = &.{
            "rhal",
            "kernel",
            "control_loop",
        },
    },
    .{
        .name = "reserve_terms",
        .members = &.{
            "declared_energy",
            "payload",
            "distance_to_safe_state",
        },
    },
    .{
        .name = "telemetry_window_faults",
        .members = &.{
            "unbounded",
            "inverted",
            "unmeasured_duration",
            "implausible_duration",
        },
    },
    .{
        .name = "upload_decisions",
        .members = &.{
            "done",
            "probe",
            "start",
            "resume",
            "restart",
            "wait_for_unmetered",
            "no_bearer",
            "refuse",
        },
    },
    .{
        .name = "upload_checks",
        .members = &.{
            "complete",
            "short",
            "long",
            "digest_mismatch",
        },
    },
    .{
        .name = "fetch_decisions",
        .members = &.{
            "have",
            "fetch",
            "resume",
            "discard_and_fetch",
            "wait_for_unmetered",
            "no_bearer",
            "refuse",
        },
    },
    .{
        .name = "bundle_checks",
        .members = &.{
            "match",
            "short",
            "long",
            "digest_mismatch",
        },
    },
    .{
        .name = "frame_refusal_codes",
        .members = &.{
            "unknown_stream",
            "unknown_slot",
            "stale_handle",
            "already_acquired",
            "double_release",
            "pool_exhausted",
        },
    },
};

/// Every set name, in declaration order, for the self-check to walk.
pub fn names(buf: [][]const u8) [][]const u8 {
    for (sets, 0..) |held, at| buf[at] = held.name;
    return buf[0..sets.len];
}

pub const count = sets.len;

/// The members of `input.set`, or JSON null for a name this edition does not hold.
///
/// Null and not an empty array, and the difference is the point: every one of the seventy-four sets has at
/// least one member, so an empty array could only ever mean *a set with nothing in it* — which would read
/// as an answer rather than as an absence. A vocabulary this edition has never heard of is the second
/// thing, and §54.2's rule about naming what could not be resolved applies to a set name as much as to a
/// path. The corpus never asks: all seventy-four of its cases name a set that exists, and a case that did
/// not would be a case this edition is right to refuse.
pub fn closedSetsCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    const wanted = value.get(input, "set") orelse return value.nul();
    if (wanted.kind != .string) return value.nul();
    for (sets) |held| {
        if (!std.mem.eql(u8, wanted.text, held.name)) continue;
        const out = try value.array(a);
        for (held.members) |member| try value.push(a, out, try value.string(a, member));
        return out;
    }
    return value.nul();
}
