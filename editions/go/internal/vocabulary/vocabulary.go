// The closed string sets, and the one place this edition spells them.
//
// `corpus/vocabulary/closed-sets.json` is the only corpus that holds an edition to a **word list**
// rather than to an answer, and it exists because of a divergence that survived a full green test
// run: a rejection code was added to the Rust edition and not to the reference, and nothing could
// see it. The mirror check guards the reference's bytes; every other corpus guards the cases it
// holds; and a member nobody had yet written a case for was outside both.
//
// # Order is part of the agreement
//
// The members are in **declaration order, not sorted**, and for several sets the order is the
// content: OSTiers and SCTSAreas are ranked, and an edition that sorted them would be one whose
// "next tier up" arithmetic silently differed while every set still held the same members.
// OperatingStates runs least-capable-first because that is the order `Evaluate` descends through
// its rules, and StateReasons is one per rule in the same sequence.
//
// # Why the sets are here rather than beside the contracts that use them
//
// Because more than one contract uses most of them, and a set defined beside its first caller is a
// set the second caller copies. §3.3: every fact exists in exactly one place. The contracts import
// what they need from here, and `Set` is the registry the corpus dispatch reads — so a set added
// to this file is answerable by the corpus without anybody wiring it up, and a set the corpus names
// that is missing here fails loudly rather than being skipped.
package vocabulary

// ── package.ts ──────────────────────────────────────────────────────────────

// PackageRejectionCodes is why an install was refused.
var PackageRejectionCodes = []string{
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
	// §85, appended.
	"capability_unknown",
}

// CapabilityDomains is the domains a canonical capability token belongs to (§85.1), in the order
// §85 lists them.
var CapabilityDomains = []string{
	"manipulation", "locomotion", "sensing", "hri", "compute", "network", "storage",
}

// CanonicalCapabilities is each domain's canonical capabilities. `sensing` is EmbodimentModalities,
// assigned in init below rather than retyped, because the reference imports it for the same reason.
var CanonicalCapabilities = map[string][]string{
	"manipulation": {"arm", "grasp", "tool", "motion"},
	"locomotion":   {"drive", "walk", "fly"},
	"hri":          {"speech", "light", "warning"},
	"compute":      {"accelerator"},
	"network":      {"outbound", "lan"},
	"storage":      {"shared"},
}

// CapabilityTokens is every canonical token, domain by domain. Filled in init.
var CapabilityTokens []string

// TokenClasses is what a string in a capability list can be (§85.1).
var TokenClasses = []string{"canonical", "unknown", "vendor", "malformed"}

func init() {
	CanonicalCapabilities["sensing"] = EmbodimentModalities
	for _, domain := range CapabilityDomains {
		for _, capability := range CanonicalCapabilities[domain] {
			CapabilityTokens = append(CapabilityTokens, domain+"."+capability)
		}
	}
	sets["capability_tokens"] = CapabilityTokens
}

// SignatureStates is what is known about a package's release signature. Deliberately not a boolean.
var SignatureStates = []string{"verified", "failed", "untrusted", "absent", "unverifiable"}

// PackageKinds is what a package can be.
var PackageKinds = []string{"policy", "skill", "driver", "service", "embodiment", "environment"}

// IsolationModes is sandboxed, or a driver.
var IsolationModes = []string{"sandboxed", "native"}

// WarningChannel is the audible warning channel (§49.1). `skynetd` owns it, it sounds on
// transition into RUNNING, and a package cannot suppress it — so the token exists to be refused.
const WarningChannel = "hri.warning"

// ReservedCapabilities is the tokens no grant confers, whoever asks and whatever the operator
// profile says. One entry today; a list rather than a comparison because the next reserved token
// will be added by somebody who found this constant.
var ReservedCapabilities = []string{WarningChannel}

// ── embodiment.ts ───────────────────────────────────────────────────────────

// TransportVerdicts is ordered worst-last; a plan takes the worst binding in it.
var TransportVerdicts = []string{"direct", "retargeted", "degraded", "refused"}

// TransportMethods is ordered by how much fidelity each costs.
var TransportMethods = []string{"direct", "remap", "retarget", "synthesize", "drop", "blocked"}

// TransportWarningCodes is everything a plan can report about a fit it accepted.
var TransportWarningCodes = []string{
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
	// §84.3, appended — the order is part of what the corpus holds editions to.
	"action_space_mismatch",
	"action_space_ambiguous",
	"motion_parameter_unused",
}

// CanonicalMotions is §84.1's motion vocabulary, in the order the section lists it.
var CanonicalMotions = []string{
	"move_joint",
	"move_linear",
	"move_circular",
	"grasp",
	"release",
	"home",
	"wait",
	"tool_change",
	"io_set",
}

// MotionFaultCodes is what can be wrong with a body's motion list (§84.1).
var MotionFaultCodes = []string{
	"unnamed",
	"duplicate_motion",
	"unknown_motion",
	"duplicate_parameter",
	"required_undeclared",
	"bound_invalid",
}

// ActionKinds is what one channel of a policy output drives.
var ActionKinds = []string{
	"joint_position",
	"joint_velocity",
	"joint_torque",
	"cartesian",
	"base_velocity",
	"gripper",
	"discrete",
}

// ControlModes is how a joint is commanded.
var ControlModes = []string{"position", "velocity", "torque", "trajectory", "cartesian"}

// JointTypes is the kinds of joint a descriptor can declare.
var JointTypes = []string{"revolute", "prismatic", "continuous", "fixed"}

// BaseKinds is what a body stands on.
var BaseKinds = []string{"fixed", "wheeled", "tracked", "legged", "aerial", "rail"}

// EmbodimentModalities is what a sensor produces.
var EmbodimentModalities = []string{
	"rgb",
	"depth",
	"stereo",
	"point_cloud",
	"imu",
	"force_torque",
	"tactile",
	"audio",
	"joint_state",
}

// CanonicalChains is the role vocabulary's chains (§5.1).
var CanonicalChains = []string{
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
}

// CanonicalAxes is the role vocabulary's axes (§5.1).
var CanonicalAxes = []string{"pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip"}

// CollisionShapes is what a collision body can be. One entry, and §44 says why.
var CollisionShapes = []string{"capsule"}

// ── calibration.ts ──────────────────────────────────────────────────────────

// CalibrationParameterSets is the sets §45.2 names, in the order that table gives them.
//
// A closed vocabulary rather than a free string, for the reason §26's event codes are closed: an
// open one is how `camera_extrinsics` and `camera-extrinsic` end up naming the same measurement on
// two robots in one fleet, and the guard mapping then answers differently for each.
var CalibrationParameterSets = []string{
	"joint_zeros", "camera_intrinsics", "camera_extrinsics", "force_torque_bias",
	"imu_bias", "tool_offset", "gravity_vector",
}

// ── safety.ts ───────────────────────────────────────────────────────────────

// OSTiers is ordered by rank. An edition that sorted these would differ on "next tier".
var OSTiers = []string{"guest", "host", "certified"}

// EnforcementModes is ordered by strength.
var EnforcementModes = []string{"advisory", "enforcing", "enforcing_redundant"}

// Guards is every guard the kernel can run.
var Guards = []string{
	"watchdog",
	"envelope",
	"rate_limit",
	"workspace",
	"self_collision",
	"balance",
	"proximity",
	"mode_gate",
}

// FaultResponses is what a tripped guard does.
var FaultResponses = []string{"clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0"}

// ── conformance.ts ──────────────────────────────────────────────────────────

// SCTSSuites is where a conformance test runs.
var SCTSSuites = []string{"sim", "bench", "field"}

// SCTSAreas is ordered as the specification lists them.
var SCTSAreas = []string{
	"rhal",
	"embodiment",
	"safety",
	"package",
	"ota",
	"telemetry",
	"offline",
	"security",
	"power",
}

// ── predicate.ts ────────────────────────────────────────────────────────────

// PredicateOps is the operators a §54.2 precondition is written in. One list, or a skill refuses on
// one robot and runs on another.
var PredicateOps = []string{
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
}

// PredicateOutcomes is satisfied, unsatisfied, and the third value §54.2 requires: a precondition
// nobody could evaluate is not a precondition that passed.
var PredicateOutcomes = []string{"satisfied", "unsatisfied", "unevaluable"}

// UnevaluableReasons is why a predicate could not be decided. §54.2 requires the refusal to name
// something, and a free-text reason is the same nothing as a free-text precondition.
var UnevaluableReasons = []string{
	"malformed",
	"unknown_op",
	"missing_path",
	"missing_operand",
	"unresolved_path",
	"not_a_number",
	"not_a_version",
}

// ── skill.ts ────────────────────────────────────────────────────────────────

// ClauseOutcomes is what can happen to one precondition or success criterion. The fourth is the one
// the predicate engine does not have: there was no predicate to ask about.
var ClauseOutcomes = []string{"satisfied", "unsatisfied", "unevaluable", "unenforceable"}

// ClauseOrigins is publisher or site. §54.2: the operator may add preconditions; the skill may not
// remove them.
var ClauseOrigins = []string{"skill", "operator"}

// PreconditionPhases is before the skill starts, and every cycle while it runs.
var PreconditionPhases = []string{"entry", "hold"}

// SkillOutcomes is how a skill invocation ends.
var SkillOutcomes = []string{"success", "failure", "aborted", "unlabeled"}

// CompositionFaults is why a call graph is refused at install (§54.5).
var CompositionFaults = []string{"cycle", "too_deep"}

// SkillPhases is §54.4's stages, in diagram order — the suite asserts the machine never moves
// backwards through this list, so an edition that sorted them would assert a different property
// with the same words.
var SkillPhases = []string{"idle", "preconditions", "acquiring", "running", "verifying", "terminal"}

// LifecycleReasons is why an invocation is where it is, in rule order.
var LifecycleReasons = []string{
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
}

// LeaseStates is §25.2's arbitration, as the one fact the lifecycle needs from it.
var LeaseStates = []string{"pending", "held", "denied"}

// ── cell.ts ─────────────────────────────────────────────────────────────────

// CellFaults is what can be wrong with a cell descriptor (§55.2).
var CellFaults = []string{
	"unnamed",
	"fact_key_unusable",
	"fact_key_reserved",
	"volume_frame_missing",
	"volume_shape_invalid",
	"fixture_unnamed",
	"duplicate_fixture",
	"parking_channel_unnamed",
	"duplicate_parking_channel",
}

// DerivedFacts is the fact keys a cell descriptor may not shadow.
var DerivedFacts = []string{"cell", "fixtures"}

// ── opstate.ts ──────────────────────────────────────────────────────────────

// OperatingStates is §27's five states plus §48.1's two, ordered least capable first — the order
// Evaluate descends through its rules.
var OperatingStates = []string{"safe", "stopping", "post", "unhomed", "hold", "degraded", "running"}

// StateReasons is why the machine is where it is. One per rule, in rule order, because two rules
// produce `hold` and only the reason tells an operator which one.
var StateReasons = []string{
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
}

// StopCauses is §27's four causes of a controlled stop, and no fifth. Closed on purpose.
var StopCauses = []string{"watchdog", "proximity", "balance", "operator", "estop"}

// PostVerdicts is §48.2's report rolled up to the one input §27's machine takes from it.
var PostVerdicts = []string{"passed", "failed", "incomplete"}

// HomingStates is whether a body knows where its joints are (§48.3).
var HomingStates = []string{"not_required", "pending", "complete"}

// RHALStates is present, degraded, lost. The last is separate from the middle because the
// difference between them is the whole of SAFE.
var RHALStates = []string{"present", "degraded", "lost"}

// PolicyStates is nothing to run, something that is not ready, and a lease granted with a plan
// resolved.
var PolicyStates = []string{"none", "held", "ready"}

// KernelStates is live or down. §27: "a robot with a policy and no kernel must not exist".
var KernelStates = []string{"live", "down"}

// ── ota.ts ──────────────────────────────────────────────────────────────────

// OSComponents is what an update bundle can carry.
var OSComponents = []string{
	"bootloader",
	"kernel_image",
	"rhal_abi",
	"rhal_driver",
	"safety_kernel",
	"envelope",
	"skynetd",
	"policy_runtime",
	"userspace",
}

// CertificationGated is the components an update may not change without a fresh bench pass.
var CertificationGated = []string{"safety_kernel", "envelope"}

// BundleFaults is what can be wrong with an update bundle.
var BundleFaults = []string{
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
}

// BootDecisions is what to do about a pending slot on boot.
var BootDecisions = []string{"not_pending", "wait", "confirm", "roll_back"}

// ── identity.ts ─────────────────────────────────────────────────────────────

// CodeFaults is why a pairing code was refused.
var CodeFaults = []string{"empty", "looks_like_a_credential", "too_short", "too_long", "not_in_alphabet"}

// DeviceScopes is what a device credential may carry.
var DeviceScopes = []string{
	"device:calibration",
	"device:checkin",
	"device:conformance",
	"device:desired",
	"device:episodes",
	"device:evaluations",
	"device:incidents",
	"device:releases",
}

// KeyBindings is where a device private key lives (§10).
var KeyBindings = []string{"hardware", "software"}

// IdentityFindings is what can be wrong with a robot's identity.
var IdentityFindings = []string{
	"not_paired",
	"namespace_missing",
	"not_a_device_credential",
	"scope_unrecognised",
	"scope_undeclared",
	"key_binding_unknown",
	"key_not_hardware_bound",
	"key_not_attested",
	"no_birth_certificate",
}

// MeasureStates is known, nothing, unmeasured — §24's three, and the difference between the last
// two is the whole of the distinction the corpus's null rule is about.
var MeasureStates = []string{"known", "nothing", "unmeasured"}

// ── birth.ts ────────────────────────────────────────────────────────────────

// BirthFindings is what can be wrong with a birth certificate.
var BirthFindings = []string{
	"unattested",
	"device_key_mismatch",
	"serial_mismatch",
	"descriptor_mismatch",
	"hardware_revision_mismatch",
	"ownership_asserted",
	"unobservable",
	"incomplete",
}

// BirthRequiredFields is what a birth certificate must carry.
var BirthRequiredFields = []string{
	"device_key",
	"model",
	"serial",
	"descriptor_digest",
	"hardware_revision",
	"manufactured",
}

// BirthOwnershipFields is what a birth certificate may not assert.
var BirthOwnershipFields = []string{"namespace", "owner", "operator", "customer"}

// ── provenance.ts ───────────────────────────────────────────────────────────

// ProvenanceClasses is how much a result is worth.
var ProvenanceClasses = []string{"field", "simulated", "unattested"}

// ProvenanceReasonCodes is why a result is classified as it is.
var ProvenanceReasonCodes = []string{
	"device_signature",
	"device_unknown",
	"simulator_declared",
	"birth_unattested",
	"image_unattested",
	"seeds_not_supplied",
	"benchmark_mismatch",
	"not_replayable",
}

// ── checkin.ts ──────────────────────────────────────────────────────────────

// CheckinClaims is what a robot claims about a desired-state generation.
var CheckinClaims = []string{"applied", "converging", "deferred", "inconsistent"}

// ReconcileStages is what a check-in reconciles, in order.
var ReconcileStages = []string{"os_image", "rhal_driver", "package", "profile", "policy", "lease"}

// ReadinessStates is what a robot reports about a stage.
var ReadinessStates = []string{"ready", "stalled", "not_running", "unknown"}

// HealthFields is what a check-in's health block may carry.
var HealthFields = []string{
	"generation_lag",
	"guard_trip_rate",
	"storage_headroom",
	"battery",
	"temperature_c",
}

// ── intent.ts ───────────────────────────────────────────────────────────────

// OfferFaults is what can be wrong with a desired-state offer.
var OfferFaults = []string{
	"undecodable",
	"generation_reused",
	"generation_went_backwards",
	"empties_the_robot",
}

// ── telemetry.ts ────────────────────────────────────────────────────────────

// TelemetryCounters is what a telemetry window counts.
var TelemetryCounters = []string{
	"cycles",
	"guard_trips",
	"clamps",
	"interventions",
	"estops",
	"package_restarts",
	"queue_dropped",
	"reconcile_passes",
}

// TelemetryWindowFaults is what can be wrong with a telemetry window.
var TelemetryWindowFaults = []string{"unbounded", "inverted", "unmeasured_duration", "implausible_duration"}

// ── power.ts ────────────────────────────────────────────────────────────────

// PowerStates is §36's states.
var PowerStates = []string{"normal", "low", "reserve", "critical", "unmetered"}

// PowerReasons is why the power state is what it is.
var PowerReasons = []string{
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
}

// PowerFaultCodes is what can be wrong with a power declaration or reading.
var PowerFaultCodes = []string{
	"gauge_silent",
	"capacity_undeclared",
	"reserve_undeclared",
	"brownout_undeclarable",
	"charge_out_of_range",
	"thermal_sustained",
	"thermal_critical",
	"thermal_undeclared",
}

// ThermalPressures is §36.3's three levels.
var ThermalPressures = []string{"nominal", "throttling", "critical"}

// ShedOrder is what is shed first, in order.
var ShedOrder = []string{
	"telemetry",
	"ota",
	"blackbox_compression",
	"inference_rate",
	"non_essential_sensors",
}

// NeverShed is what is never shed, whatever the reserve says.
var NeverShed = []string{"rhal", "kernel", "control_loop"}

// ReserveTerms is what the reserve budget is computed from.
var ReserveTerms = []string{"declared_energy", "payload", "distance_to_safe_state"}

// ── upload.ts and bundle.ts ─────────────────────────────────────────────────

// UploadDecisions is what to do about an upload.
var UploadDecisions = []string{
	"done",
	"probe",
	"start",
	"resume",
	"restart",
	"wait_for_unmetered",
	"no_bearer",
	"refuse",
}

// UploadChecks is what a resumed upload's probe found.
var UploadChecks = []string{"complete", "short", "long", "digest_mismatch"}

// FetchDecisions is what to do about a bundle fetch.
var FetchDecisions = []string{
	"have",
	"fetch",
	"resume",
	"discard_and_fetch",
	"wait_for_unmetered",
	"no_bearer",
	"refuse",
}

// BundleChecks is what a resumed fetch's probe found.
var BundleChecks = []string{"match", "short", "long", "digest_mismatch"}

// ── media.ts ────────────────────────────────────────────────────────────────

// FrameRefusalCodes is why a frame handle was refused.
var FrameRefusalCodes = []string{
	"unknown_stream",
	"unknown_slot",
	"stale_handle",
	"already_acquired",
	"double_release",
	"pool_exhausted",
}

// ── the registry ────────────────────────────────────────────────────────────

// sets is the name the corpus asks for, to the members.
//
// A map is right here where it is wrong for a JSON object: nothing iterates it, every read is by
// name, and the values it holds are the slices above — whose order is preserved because the slice
// is the value, not a copy the map made.
var sets = map[string][]string{
	"package_rejection_codes": PackageRejectionCodes,
	"signature_states":        SignatureStates,
	"package_kinds":           PackageKinds,
	"isolation_modes":         IsolationModes,
	"reserved_capabilities":   ReservedCapabilities,
	"capability_domains":      CapabilityDomains,
	"token_classes":           TokenClasses,
	"transport_verdicts":      TransportVerdicts,
	"transport_methods":       TransportMethods,
	"transport_warning_codes": TransportWarningCodes,
	"action_kinds":            ActionKinds,
	"control_modes":           ControlModes,
	"joint_types":             JointTypes,
	"base_kinds":              BaseKinds,
	"embodiment_modalities":   EmbodimentModalities,
	"canonical_chains":        CanonicalChains,
	"canonical_axes":          CanonicalAxes,
	"canonical_motions":       CanonicalMotions,
	"motion_fault_codes":      MotionFaultCodes,
	"os_tiers":                OSTiers,
	"enforcement_modes":       EnforcementModes,
	"guards":                  Guards,
	"fault_responses":         FaultResponses,
	"scts_suites":             SCTSSuites,
	"scts_areas":              SCTSAreas,
	"predicate_ops":           PredicateOps,
	"predicate_outcomes":      PredicateOutcomes,
	"unevaluable_reasons":     UnevaluableReasons,
	"clause_outcomes":         ClauseOutcomes,
	"clause_origins":          ClauseOrigins,
	"precondition_phases":     PreconditionPhases,
	"skill_outcomes":          SkillOutcomes,
	"composition_faults":      CompositionFaults,
	"cell_faults":             CellFaults,
	"operating_states":        OperatingStates,
	"state_reasons":           StateReasons,
	"stop_causes":             StopCauses,
	"post_verdicts":           PostVerdicts,
	"homing_states":           HomingStates,
	"rhal_states":             RHALStates,
	"policy_states":           PolicyStates,
	"kernel_states":           KernelStates,
	"skill_phases":            SkillPhases,
	"lifecycle_reasons":       LifecycleReasons,
	"lease_states":            LeaseStates,
	"os_components":           OSComponents,
	"certification_gated":     CertificationGated,
	"bundle_faults":           BundleFaults,
	"boot_decisions":          BootDecisions,
	"code_faults":             CodeFaults,
	"device_scopes":           DeviceScopes,
	"key_bindings":            KeyBindings,
	"identity_findings":       IdentityFindings,
	"measure_states":          MeasureStates,
	"birth_findings":          BirthFindings,
	"birth_required_fields":   BirthRequiredFields,
	"birth_ownership_fields":  BirthOwnershipFields,
	"provenance_classes":      ProvenanceClasses,
	"provenance_reason_codes": ProvenanceReasonCodes,
	"checkin_claims":          CheckinClaims,
	"reconcile_stages":        ReconcileStages,
	"readiness_states":        ReadinessStates,
	"health_fields":           HealthFields,
	"offer_faults":            OfferFaults,
	"telemetry_counters":      TelemetryCounters,
	"power_states":            PowerStates,
	"power_reasons":           PowerReasons,
	"power_fault_codes":       PowerFaultCodes,
	"thermal_pressures":       ThermalPressures,
	"shed_order":              ShedOrder,
	"never_shed":              NeverShed,
	"reserve_terms":           ReserveTerms,
	"telemetry_window_faults": TelemetryWindowFaults,
	"upload_decisions":        UploadDecisions,
	"upload_checks":           UploadChecks,
	"fetch_decisions":         FetchDecisions,
	"bundle_checks":           BundleChecks,
	"frame_refusal_codes":     FrameRefusalCodes,
}

// Set returns the named vocabulary and whether this edition has it.
//
// The bool is not decoration. corpus/README.md: *"a set the corpus names that an edition does not
// have is reported as an error rather than skipped, because that is exactly the divergence being
// looked for."*
func Set(name string) ([]string, bool) {
	members, found := sets[name]
	return members, found
}

// Contains reports whether a member is in a set. The contracts ask this constantly — is this a
// known guard, a declared scope, a reserved token — and one spelling of the loop means one place
// for it to be wrong.
func Contains(members []string, value string) bool {
	for _, member := range members {
		if member == value {
			return true
		}
	}
	return false
}

// IndexOf returns the position of a member, or -1. Rank, for the sets whose order is their content:
// the tier ladder and the enforcement ladder are compared by this rather than by a chain of ifs.
func IndexOf(members []string, value string) int {
	for at, member := range members {
		if member == value {
			return at
		}
	}
	return -1
}
