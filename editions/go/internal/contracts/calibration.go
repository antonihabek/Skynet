package contracts

import (
	"math"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// Calibration validity (§45).
//
// A parameter set with no record at all is `absent`, and its guards are unavailable for that
// reason. The alternative — treating "never calibrated" as "no evidence of a problem" — is what
// puts a `proximity` guard on a robot whose camera transform is whatever the CAD model said.

// guardsFedByTable is §45.2's table, as data: which guards lose their input when a set expires.
//
// **This table is the deliverable of §45.2.** It converts "we should calibrate the cameras
// sometime" into "an expired extrinsic disables the proximity guard", which is a sentence an
// operator can act on and a fleet console can count.
//
// `camera_intrinsics` feeds `proximity` *indirectly*, through projection, and is listed rather than
// omitted: a guard whose input is two transforms away is still measuring the wrong thing when
// either is wrong.
//
// A slice of pairs rather than a map, because the values are ordered lists that reach the wire and
// a Go map would randomise any iteration of the keys.
var guardsFedByTable = []struct {
	set    string
	guards []string
}{
	{"joint_zeros", []string{"workspace", "self_collision"}},
	{"camera_intrinsics", []string{"proximity"}},
	{"camera_extrinsics", []string{"proximity", "workspace"}},
	{"force_torque_bias", []string{"envelope"}},
	{"imu_bias", []string{"balance"}},
	{"tool_offset", []string{"workspace"}},
	{"gravity_vector", []string{"balance", "envelope"}},
}

// GuardsFedBy is the guards a parameter set feeds, or none.
func GuardsFedBy(set string) []string {
	for _, entry := range guardsFedByTable {
		if entry.set == set {
			return entry.guards
		}
	}
	return []string{}
}

// Validity is what judging one record concluded.
type Validity struct {
	State string
	// Detail is empty for `valid`, and the reference renders `absent` with a phrase of its own
	// rather than an empty string — see ValidityDetail.
	Detail string
	// Reason is set only for `expired`.
	Reason string
	// Checked is set only for `valid`: the conditions this record was actually judged against.
	// Empty is load-bearing — it is how a console tells "still valid" from "nothing was checked".
	Checked []string
}

// ValidityDetail is phrased to compose: it is read on its own in a table row that already names the
// set, and inside "the `set` calibration is absent — …" in the guard list.
func ValidityDetail(validity Validity) string {
	switch validity.State {
	case "valid":
		return ""
	case "absent":
		return "nobody has ever measured it on this robot"
	}
	return validity.Detail
}

// EvaluateCalibration judges one record against the present (§45.4, §45.5).
//
// # Order, and why it is this order
//
//  1. **Provenance**, because §45.3 makes it mandatory and a record without a method cannot be
//     judged on anything else — "valid for another 30 days" would be a claim about a measurement
//     nobody described.
//  2. **Residual**, because a failed calibration was never valid and its age is irrelevant.
//  3. **Events**, because §45.4 calls them the triggers that matter and they are a step rather than
//     a slope: a robot that hit something has a bad extrinsic *now*, whatever its age says.
//  4. Then the three gradual conditions, in the order §45.4's table gives them.
//
// A missing *current* reading does not extend validity. If the policy names a temperature delta and
// the robot cannot say what temperature it is, the record expires — the alternative is a rule that
// silently stops applying on exactly the robot whose sensor has failed.
func EvaluateCalibration(record canonical.Value, now canonical.Value) Validity {
	policy := record.Field("policy")
	taken := record.Field("conditions")

	missing := []string{}
	if !js.Truthy(stringOr(record.Field("method"), "")) {
		missing = append(missing, "method")
	}
	residual, hasResidual := finiteNumber(record.Field("residual"))
	if !hasResidual {
		missing = append(missing, "residual")
	}
	if len(missing) > 0 {
		return Validity{
			State: "unusable",
			Detail: "§45.3 makes provenance mandatory and this record has no " +
				strings.Join(missing, " and no ") + ". A calibration whose method nobody recorded is a rumour " +
				"with six decimal places",
		}
	}

	threshold, hasThreshold := finiteNumber(record.Field("residual_threshold"))
	if hasResidual && hasThreshold && residual > threshold {
		return Validity{
			State: "failed",
			Detail: "the method reported a residual of " + js.NumberToString(residual) +
				" against its own threshold of " + js.NumberToString(threshold) +
				", so this was a failed calibration rather than one that has since expired",
		}
	}

	invalidating := stringsOf(policy.Field("invalidating_events"))
	for _, event := range stringsOf(record.Field("events_since")) {
		if containsString(invalidating, event) {
			return Validity{
				State:  "expired",
				Reason: "event",
				Detail: "a `" + event + "` was recorded against this set since it was measured, and §45.4 " +
					"invalidates it outright: continuing to trust the previous values is exactly the wrong " +
					"response to one",
			}
		}
	}

	checked := []string{}

	if maxAge, has := finiteNumber(policy.Field("max_age_s")); has {
		takenAt, hasTaken := finiteNumber(taken.Field("at_s"))
		nowAt, hasNow := finiteNumber(now.Field("at_s"))
		if !hasTaken || !hasNow {
			return Validity{
				State: "expired", Reason: "age",
				Detail: "this record expires with time and either it or this robot cannot say when it was " +
					"taken, so its age cannot be established",
			}
		}
		age := nowAt - takenAt
		if age > maxAge {
			return Validity{
				State: "expired", Reason: "age",
				Detail: "measured " + js.ToFixed(age, 0) + " s ago against a validity of " +
					js.ToFixed(maxAge, 0) + " s",
			}
		}
		checked = append(checked, "age "+js.ToFixed(age, 0)+"s of "+js.ToFixed(maxAge, 0)+"s")
	}

	if maxTravel, has := finiteNumber(policy.Field("max_joint_travel_rad")); has {
		takenTravel, hasTaken := finiteNumber(taken.Field("joint_travel_rad"))
		nowTravel, hasNow := finiteNumber(now.Field("joint_travel_rad"))
		if !hasTaken || !hasNow {
			return Validity{
				State: "expired", Reason: "joint_travel",
				Detail: "this record expires with joint travel and no travel counter is available, so how " +
					"far this robot has moved since cannot be established",
			}
		}
		travelled := nowTravel - takenTravel
		if travelled > maxTravel {
			return Validity{
				State: "expired", Reason: "joint_travel",
				Detail: js.ToFixed(travelled, 1) + " rad of joint travel since it was measured, against a " +
					"validity of " + js.ToFixed(maxTravel, 1) + " rad",
			}
		}
		checked = append(checked, "travel "+js.ToFixed(travelled, 1)+" of "+js.ToFixed(maxTravel, 1)+" rad")
	}

	if maxDelta, has := finiteNumber(policy.Field("max_temperature_delta_c")); has {
		takenTemperature, hasTaken := finiteNumber(taken.Field("temperature_c"))
		nowTemperature, hasNow := finiteNumber(now.Field("temperature_c"))
		if !hasTaken || !hasNow {
			return Validity{
				State: "expired", Reason: "temperature",
				Detail: "this record expires with temperature and no reading is available, so how far this " +
					"robot has drifted from its calibration conditions cannot be established",
			}
		}
		delta := math.Abs(nowTemperature - takenTemperature)
		if delta > maxDelta {
			return Validity{
				State: "expired", Reason: "temperature",
				Detail: js.ToFixed(delta, 1) + " °C from the " + js.ToFixed(takenTemperature, 1) +
					" °C it was measured at, against a validity of " + js.ToFixed(maxDelta, 1) + " °C",
			}
		}
		checked = append(checked, "temperature Δ"+js.ToFixed(delta, 1)+" of "+js.ToFixed(maxDelta, 1)+" °C")
	}

	return Validity{State: "valid", Checked: checked}
}

// GuardUnavailable is one guard that has lost its input.
type GuardUnavailable struct {
	Guard  string
	Set    string
	State  string
	Detail string
}

// GuardsUnavailable is which guards lose their input, given what has been measured (§45.5).
//
// One entry per (guard, set) pair, deliberately: `workspace` disabled by expired joint zeros *and*
// an expired tool offset is two measurements somebody has to take, and collapsing them to one line
// would hide the second until the first was done.
//
// `applicable` names the sets that mean anything on this body — a fixed-base arm with no IMU should
// not report `balance` unavailable for want of an IMU bias it can never have.
func GuardsUnavailable(setNames []string, validities []Validity, applicable []string) []GuardUnavailable {
	out := []GuardUnavailable{}
	for _, set := range applicable {
		validity := Validity{State: "absent"}
		for at, name := range setNames {
			if name == set {
				validity = validities[at]
				break
			}
		}
		if validity.State == "valid" {
			continue
		}
		for _, guard := range GuardsFedBy(set) {
			out = append(out, GuardUnavailable{
				Guard: guard, Set: set, State: validity.State,
				Detail: "the `" + set + "` calibration is " + validity.State + " — " + ValidityDetail(validity),
			})
		}
	}
	return out
}

// SetsFor is the sets a body can actually hold, from what it has declared.
//
// # Two different absences, and they are not treated the same
//
// **Undeclared capabilities** mean nobody said what this body senses, so every sensing-derived set
// is included: over-reporting what is missing is the conservative direction, and quietly deciding a
// guard was never relevant is not.
//
// **A declared `fixed` base** is not an absence at all — it is a positive statement that this body
// cannot fall over. `balance` is meaningless on a bolted-down arm, so an IMU bias and a gravity
// vector are not measurements it is missing; they are measurements it will never take, and
// reporting `balance` unavailable on one is a permanent red mark nobody can clear.
//
// An **unknown** base is mobile, because that is the absence again and it goes the other way.
func SetsFor(capabilities []string, base string, hasBase bool) []string {
	mobile := !hasBase || base != "fixed"
	declared := len(capabilities) > 0
	has := func(prefix string) bool {
		for _, capability := range capabilities {
			if strings.HasPrefix(capability, prefix) {
				return true
			}
		}
		return false
	}

	sets := []string{"joint_zeros"}
	if !declared || has("sensing.rgb") || has("sensing.depth") || has("sensing.camera") {
		sets = append(sets, "camera_intrinsics", "camera_extrinsics")
	}
	if !declared || has("sensing.force") || has("manipulation") {
		sets = append(sets, "force_torque_bias", "tool_offset")
	}
	if mobile || has("sensing.imu") {
		sets = append(sets, "imu_bias", "gravity_vector")
	}
	return sets
}

// ── the corpus ──────────────────────────────────────────────────────────────

func calibrationCase(input canonical.Value) (canonical.Value, error) {
	now := input.Field("now")
	setNames := []string{}
	validities := []Validity{}
	for _, record := range input.Field("records").Items() {
		set, _ := record.Field("set").Str()
		setNames = append(setNames, set)
		validities = append(validities, EvaluateCalibration(record, now))
	}

	// `applicable ?? allSets()`: absent means every parameter set, which is the reading for a robot
	// whose body does not say which apply.
	applicable := vocabulary.CalibrationParameterSets
	if declared, present := input.Get("applicable"); present && declared.Kind() == canonical.KindArray {
		applicable = stringsOf(declared)
	}

	records := canonical.Array()
	for at, set := range setNames {
		validity := validities[at]
		reason := canonical.Null()
		if validity.State == "expired" {
			reason = canonical.String(validity.Reason)
		}
		checked := []string{}
		if validity.State == "valid" {
			checked = validity.Checked
		}
		records = records.Push(canonical.Object().
			Set("set", canonical.String(set)).
			Set("state", canonical.String(validity.State)).
			Set("detail", canonical.String(ValidityDetail(validity))).
			Set("reason", reason).
			Set("checked", canonical.Strings(checked)).
			Set("guards", canonical.Strings(GuardsFedBy(set))))
	}

	unavailable := canonical.Array()
	for _, entry := range GuardsUnavailable(setNames, validities, applicable) {
		unavailable = unavailable.Push(canonical.Object().
			Set("guard", canonical.String(entry.Guard)).
			Set("set", canonical.String(entry.Set)).
			Set("state", canonical.String(entry.State)).
			Set("detail", canonical.String(entry.Detail)))
	}

	return canonical.Object().
		Set("records", records).
		Set("unavailable", unavailable), nil
}

func calibrationSets(input canonical.Value) (canonical.Value, error) {
	base, hasBase := input.Field("base").Str()
	return canonical.Object().
		Set("sets", canonical.Strings(SetsFor(stringsOf(input.Field("capabilities")), base, hasBase))), nil
}
