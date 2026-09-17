#include "skynet_os/vocabulary.hpp"

#include "skynet_os/json.hpp"

#include <algorithm>
#include <string>
#include <utility>

namespace skynet::vocabulary {
namespace {

/// Every closed set, keyed by the name the corpus uses, in the reference edition's declaration
/// order. The comment beside each names the file it was transcribed from, so the next person to
/// change one knows which source has to change first.
const std::vector<std::pair<std::string_view, Set>>& table() {
  static const std::vector<std::pair<std::string_view, Set>> sets = {
      // package.ts
      {"package_rejection_codes",
       {"unsigned", "signature_failed", "signature_untrusted", "signature_unverifiable",
        "digest_mismatch", "isolation_violation", "budget_exceeded", "capability_ungranted",
        "capability_denied", "capability_reserved", "over_request", "missing_dependency",
        "os_too_old", "transport_refused", "executes_on_load_untrusted", "environment_inert",
        "embodiment_undeclared", "capability_unknown"}},
      {"signature_states", {"verified", "failed", "untrusted", "absent", "unverifiable"}},
      {"package_kinds", {"policy", "skill", "driver", "service", "embodiment", "environment"}},
      {"isolation_modes", {"sandboxed", "native"}},
      // package.ts: `RESERVED_CAPABILITIES = [WARNING_CHANNEL]`, one token. A regular expression
      // that ran past the end of the declaration once read this as eight members; it is one.
      {"reserved_capabilities", {"hri.warning"}},
      // package.ts, §85.1 — domains in the order the section lists them, then every token.
      // Membership is the whole rule, and not every token has two segments.
      {"capability_domains",
       {"locomotion", "manipulation", "sensing", "control", "safety", "platform", "hri"}},
      {"capability_tokens",
       {"locomotion.walk", "locomotion.velocity", "locomotion.stairs", "locomotion.turn_in_place",
        "locomotion.stand", "locomotion.fly", "manipulation.arm", "manipulation.dual_arm",
        "manipulation.grasp", "manipulation.dexterous_hand", "manipulation.cartesian",
        "manipulation.force_control", "manipulation.mobile", "sensing.camera.rgb",
        "sensing.camera.depth", "sensing.camera.stereo", "sensing.lidar", "sensing.imu",
        "sensing.force_torque", "sensing.tactile", "sensing.joint_state", "sensing.odometry",
        "sensing.audio", "control.position", "control.velocity", "control.torque",
        "control.trajectory", "control.realtime", "control.motion", "safety.estop",
        "safety.collision_avoidance", "safety.workspace_limits", "platform.onboard_compute",
        "platform.ros2", "platform.teleop", "platform.data_recording", "hri.speech", "hri.warning"}},
      {"token_classes", {"canonical", "unknown", "vendor", "malformed"}},

      // embodiment.ts
      {"transport_verdicts", {"direct", "retargeted", "degraded", "refused"}},
      {"transport_methods", {"direct", "remap", "retarget", "synthesize", "drop", "blocked"}},
      {"transport_warning_codes",
       {"no_source_embodiment", "unverified_embodiment", "no_target_envelope", "dof_mismatch",
        "control_mode_unsupported", "rate_unsupported", "rate_below_watchdog", "scaled_down",
        "unscalable_channel", "retarget_without_solver", "unmapped_roles", "observation_missing",
        "observation_degraded", "latency_undeclared", "action_space_mismatch",
        "action_space_ambiguous", "motion_parameter_unused",
        // §86, appended for the reason every other addition here is.
        "joint_mode_conflict", "mixed_modes_unsupported"}},
      {"action_kinds",
       {"joint_position", "joint_velocity", "joint_torque", "cartesian", "base_velocity", "gripper",
        "discrete"}},
      {"control_modes", {"position", "velocity", "torque", "trajectory", "cartesian"}},
      {"joint_types", {"revolute", "prismatic", "continuous", "fixed"}},
      {"base_kinds", {"fixed", "wheeled", "tracked", "legged", "aerial", "rail"}},
      {"embodiment_modalities",
       {"rgb", "depth", "stereo", "point_cloud", "imu", "force_torque", "tactile", "audio",
        "joint_state"}},
      {"canonical_chains",
       {"left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand", "torso",
        "neck", "head", "base", "tail", "aux"}},
      {"canonical_axes", {"pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip"}},
      {"canonical_motions",
       {"move_joint", "move_linear", "move_circular", "grasp", "release", "home", "wait",
        "tool_change", "io_set"}},
      {"motion_fault_codes",
       {"unnamed", "duplicate_motion", "unknown_motion", "duplicate_parameter",
        "required_undeclared", "bound_invalid"}},

      // safety.ts — OS_TIERS in rank order and FAULT_RESPONSES least to most severe.
      {"os_tiers", {"guest", "host", "certified"}},
      {"enforcement_modes", {"advisory", "enforcing", "enforcing_redundant"}},
      {"guards",
       {"watchdog", "envelope", "rate_limit", "workspace", "self_collision", "balance", "proximity",
        "mode_gate"}},
      {"fault_responses", {"clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0"}},

      // conformance.ts
      {"scts_suites", {"sim", "bench", "field"}},
      {"scts_areas",
       {"rhal", "embodiment", "safety", "package", "ota", "telemetry", "offline", "security",
        "power"}},

      // predicate.ts
      {"predicate_ops",
       {"and", "or", "not", "eq", "neq", "in", "gte", "lte", "gt", "lt", "includes", "includes_all",
        "includes_any", "exists", "semver_satisfies"}},
      {"predicate_outcomes", {"satisfied", "unsatisfied", "unevaluable"}},
      {"unevaluable_reasons",
       {"malformed", "unknown_op", "missing_path", "missing_operand", "unresolved_path",
        "not_a_number", "not_a_version"}},

      // skill.ts
      {"clause_outcomes", {"satisfied", "unsatisfied", "unevaluable", "unenforceable"}},
      {"clause_origins", {"skill", "operator"}},
      {"precondition_phases", {"entry", "hold"}},
      {"skill_outcomes", {"success", "failure", "aborted", "unlabeled"}},
      {"composition_faults", {"cycle", "too_deep"}},

      // cell.ts
      {"cell_faults",
       {"unnamed", "fact_key_unusable", "fact_key_reserved", "volume_frame_missing",
        "volume_shape_invalid", "fixture_unnamed", "duplicate_fixture", "parking_channel_unnamed",
        "duplicate_parking_channel"}},

      // opstate.ts
      {"operating_states", {"safe", "stopping", "post", "unhomed", "hold", "degraded", "running"}},
      {"state_reasons",
       {"rhal_lost", "safe_sticky", "post_failed", "stop_complete", "stopping_watchdog",
        "stopping_proximity", "stopping_balance", "stopping_operator", "stopping_estop",
        "post_incomplete", "homing_pending", "kernel_down", "guard_hold", "no_policy",
        "policy_not_ready", "guards_unavailable", "rhal_degraded", "nominal"}},
      {"stop_causes", {"watchdog", "proximity", "balance", "operator", "estop"}},
      {"post_verdicts", {"passed", "failed", "incomplete"}},
      {"homing_states", {"not_required", "pending", "complete"}},
      {"rhal_states", {"present", "degraded", "lost"}},
      {"policy_states", {"none", "held", "ready"}},
      {"kernel_states", {"live", "down"}},

      // skill.ts, the lifecycle half
      {"skill_phases", {"idle", "preconditions", "acquiring", "running", "verifying", "terminal"}},
      {"lifecycle_reasons",
       {"not_invoked", "invoked", "preconditions_pending", "preconditions_refused",
        "preconditions_met", "leases_held_by_root", "lease_pending", "lease_denied",
        "power_withheld", "leases_acquired", "timeout", "hold_lost", "guard_tripped",
        "robot_stopped", "running", "verifying", "verdict_pending", "judged", "terminal"}},
      {"lease_states", {"pending", "held", "denied"}},

      // ota.ts
      {"os_components",
       {"bootloader", "kernel_image", "rhal_abi", "rhal_driver", "safety_kernel", "envelope",
        "skynetd", "policy_runtime", "userspace"}},
      {"certification_gated", {"safety_kernel", "envelope"}},
      {"bundle_faults",
       {"unversioned", "undigested", "signature_failed", "signature_untrusted", "signature_absent",
        "signature_unverifiable", "incompatible", "compatibility_undeclared", "channel_mismatch",
        "certification_required", "bench_pass_stale", "bench_pass_failed", "already_running"}},
      {"boot_decisions", {"not_pending", "wait", "confirm", "roll_back"}},

      // identity.ts
      {"code_faults",
       {"empty", "looks_like_a_credential", "too_short", "too_long", "not_in_alphabet"}},
      {"device_scopes",
       {"device:calibration", "device:checkin", "device:conformance", "device:desired",
        "device:episodes", "device:evaluations", "device:incidents", "device:releases"}},
      {"key_bindings", {"hardware", "software"}},
      {"identity_findings",
       {"not_paired", "namespace_missing", "not_a_device_credential", "scope_unrecognised",
        "scope_undeclared", "key_binding_unknown", "key_not_hardware_bound", "key_not_attested",
        "no_birth_certificate"}},
      {"measure_states", {"known", "nothing", "unmeasured"}},

      // birth.ts
      {"birth_findings",
       {"unattested", "device_key_mismatch", "serial_mismatch", "descriptor_mismatch",
        "hardware_revision_mismatch", "ownership_asserted", "unobservable", "incomplete"}},
      {"birth_required_fields",
       {"device_key", "model", "serial", "descriptor_digest", "hardware_revision", "manufactured"}},
      {"birth_ownership_fields", {"namespace", "owner", "operator", "customer"}},

      // provenance.ts
      {"provenance_classes", {"field", "simulated", "unattested"}},
      {"provenance_reason_codes",
       {"device_signature", "device_unknown", "simulator_declared", "birth_unattested",
        "image_unattested", "seeds_not_supplied", "benchmark_mismatch", "not_replayable"}},

      // checkin.ts
      {"checkin_claims", {"applied", "converging", "deferred", "inconsistent"}},
      {"reconcile_stages", {"os_image", "rhal_driver", "package", "profile", "policy", "lease"}},
      {"readiness_states", {"ready", "stalled", "not_running", "unknown"}},
      {"health_fields",
       {"generation_lag", "guard_trip_rate", "storage_headroom", "battery", "temperature_c"}},

      // intent.ts
      {"offer_faults",
       {"undecodable", "generation_reused", "generation_went_backwards", "empties_the_robot"}},

      // telemetry.ts
      {"telemetry_counters",
       {"cycles", "guard_trips", "clamps", "interventions", "estops", "package_restarts",
        "queue_dropped", "reconcile_passes"}},

      // power.ts — SHED_ORDER is the order things are shed in, so it is not sorted either.
      {"power_states", {"normal", "low", "reserve", "critical", "unmetered"}},
      {"power_reasons",
       {"brownout", "below_reserve", "at_reserve", "approaching_reserve", "above_reserve", "no_pack",
        "no_reading", "no_reserve", "no_capacity", "reading_unusable"}},
      {"power_fault_codes",
       {"gauge_silent", "capacity_undeclared", "reserve_undeclared", "brownout_undeclarable",
        "charge_out_of_range", "thermal_sustained", "thermal_critical", "thermal_undeclared"}},
      {"thermal_pressures", {"nominal", "throttling", "critical"}},
      {"shed_order",
       {"telemetry", "ota", "blackbox_compression", "inference_rate", "non_essential_sensors"}},
      {"never_shed", {"rhal", "kernel", "control_loop"}},
      {"reserve_terms", {"declared_energy", "payload", "distance_to_safe_state"}},

      // telemetry.ts, the window half
      {"telemetry_window_faults",
       {"unbounded", "inverted", "unmeasured_duration", "implausible_duration"}},

      // upload.ts
      {"upload_decisions",
       {"done", "probe", "start", "resume", "restart", "wait_for_unmetered", "no_bearer",
        "refuse"}},
      {"upload_checks", {"complete", "short", "long", "digest_mismatch"}},

      // bundle.ts
      {"fetch_decisions",
       {"have", "fetch", "resume", "discard_and_fetch", "wait_for_unmetered", "no_bearer",
        "refuse"}},
      {"bundle_checks", {"match", "short", "long", "digest_mismatch"}},

      // media.ts — §47.6.
      {"frame_refusal_codes",
       {"unknown_stream", "unknown_slot", "stale_handle", "already_acquired", "double_release",
        "pool_exhausted"}},
  };
  return sets;
}

}  // namespace

const Set& members(std::string_view name) {
  const auto& sets = table();
  const auto found = std::find_if(sets.begin(), sets.end(),
                                  [name](const auto& entry) { return entry.first == name; });
  if (found == sets.end()) {
    throw JsonError(std::string(name) + " is not a closed set in this vocabulary");
  }
  return found->second;
}

bool contains(std::string_view name, std::string_view candidate) {
  const Set& set = members(name);
  return std::find(set.begin(), set.end(), candidate) != set.end();
}

std::size_t count() { return table().size(); }

}  // namespace skynet::vocabulary
