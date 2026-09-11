package contracts

import (
	"sort"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// §54.2's live precondition context, assembled — the document a skill's preconditions read.
//
// One rule decides every field: **absent is not null**. A fact this robot does not know is absent
// from the context, never present as null and never as a plausible zero. A robot with no battery
// sensor reporting 0 would refuse every skill and one reporting 100 would run every skill; one that
// omits the path refuses only the skills that asked, and names the path in the refusal.
//
// Two branches are sorted lists rather than objects, because a capability token contains a dot by
// construction and a dot path cannot address it: grants are `skill.grants.tokens` and frames are
// `robot.frames.resolved`, both read with `includes`. Joint names stay keys, because the question
// about a joint is its value; a joint whose name no dot path can address is omitted and reported.

// addressable is a key a dot path can address: non-empty, no dot, and no leading or trailing
// whitespace.
//
// The same rule as cell.go's usableKey, and deliberately written twice rather than shared, because
// the reference writes it twice for a reason: this one is about a name the **body** chose and that
// one a name the **operator** chose, and the two could reasonably diverge about whitespace without
// either being wrong. Delegating to usableKey would make a change to the cell rule change this one in
// this edition alone.
func addressable(key string) bool {
	return key != "" && !containsByte(key, '.') && js.Trim(key) == key
}

// sortedUnique is the reference's `[...new Set(values)].sort()`: deduplicated, then ordered by
// UTF-16 code unit as a default JavaScript sort compares strings.
func sortedUnique(values []string) []string {
	seen := map[string]bool{}
	out := []string{}
	for _, value := range values {
		if !seen[value] {
			seen[value] = true
			out = append(out, value)
		}
	}
	sort.SliceStable(out, func(left, right int) bool { return utf16Less(out[left], out[right]) })
	return out
}

// LiveContext is the context, plus what it could not say and why.
type LiveContext struct {
	Context canonical.Value
	// Unaddressable are joint and set names no dot path can address, reported rather than silently
	// dropped: the symptom otherwise is an unresolvable path and an operator who can see the joint.
	Unaddressable []string
	// Unreadable are readings that were not finite, and are therefore not readings.
	Unreadable []string
}

// BuildLiveContext builds §54.2's context from what the OS knows.
//
// Every branch is omitted rather than emitted empty when nothing is known about it, with one
// exception: `skill.grants.tokens` is always present, because a grant is a file the daemon owns and
// "this package was granted nothing" is an answer the OS always has.
func BuildLiveContext(inputs canonical.Value) LiveContext {
	unaddressable := []string{}
	unreadable := []string{}

	// ── robot.state ──
	joints := canonical.Object()
	for _, reading := range inputs.Field("joints").Items() {
		name, _ := reading.Field("name").Str()
		if !addressable(name) {
			unaddressable = append(unaddressable, name)
			continue
		}
		position := reading.Field("position")
		if !finiteNumberValue(position) {
			unreadable = append(unreadable, name)
			continue
		}
		joints = joints.Set(name, position)
	}

	state := canonical.Object()
	if joints.Len() > 0 {
		state = state.Set("joints", joints)
	}
	for _, key := range []string{"gripper", "battery_percent"} {
		if value := inputs.Field(key); finiteNumberValue(value) {
			state = state.Set(key, value)
		}
	}
	if pose := inputs.Field("base_pose"); truthyValue(pose) {
		xyz, quaternion := pose.Field("xyz"), pose.Field("quaternion")
		if everyFinite(xyz) && everyFinite(quaternion) {
			shaped := canonical.Object()
			for at, key := range []string{"x", "y", "z"} {
				if at < xyz.Len() {
					shaped = shaped.Set(key, xyz.At(at))
				}
			}
			for at, key := range []string{"qx", "qy", "qz", "qw"} {
				if at < quaternion.Len() {
					shaped = shaped.Set(key, quaternion.At(at))
				}
			}
			state = state.Set("base_pose", shaped)
		} else {
			unreadable = append(unreadable, "base_pose")
		}
	}
	// §27's state, under its own name so the path stays stable if the machine grows a second field.
	if operating := inputs.Field("operating_state"); truthyValue(operating) {
		state = state.Set("operating", operating)
	}

	// ── robot.guards ──
	// `if (inputs.guards_active)`: an empty list is truthy and is published — a kernel reporting that
	// no guard runs — while absent is nobody having asked a kernel, and publishes nothing.
	guards := canonical.Object()
	if active := inputs.Field("guards_active"); truthyValue(active) {
		guards = guards.Set("active", canonical.Strings(sortedUnique(stringsOf(active))))
	}
	if unavailable := stringsOf(inputs.Field("guards_unavailable")); len(unavailable) > 0 {
		guards = guards.Set("unavailable", canonical.Strings(sortedUnique(unavailable)))
	}

	// ── robot.calibration ──
	calibration := canonical.Object()
	for _, fact := range inputs.Field("calibration").Items() {
		set, _ := fact.Field("set").Str()
		if !addressable(set) {
			unaddressable = append(unaddressable, set)
			continue
		}
		validityState, present := fact.Field("validity").Get("state")
		text, _ := validityState.Str()
		row := canonical.Object().
			Set("valid", canonical.Bool(text == "valid")).
			SetIf("state", validityState, present)
		if age := fact.Field("age_s"); finiteNumberValue(age) {
			row = row.Set("age_s", age)
		}
		calibration = calibration.Set(set, row)
	}

	// ── robot ──
	robot := canonical.Object()
	if state.Len() > 0 {
		robot = robot.Set("state", state)
	}
	if guards.Len() > 0 {
		robot = robot.Set("guards", guards)
	}
	if calibration.Len() > 0 {
		robot = robot.Set("calibration", calibration)
	}
	frames := canonical.Object()
	if resolved := stringsOf(inputs.Field("frames_resolved")); len(resolved) > 0 {
		frames = frames.Set("resolved", canonical.Strings(sortedUnique(resolved)))
	}
	if fault := inputs.Field("frames_fault"); truthyValue(fault) {
		code, hasCode := fault.Get("code")
		detail, hasDetail := fault.Get("detail")
		frames = frames.Set("fault", canonical.Object().
			SetIf("code", code, hasCode).
			SetIf("detail", detail, hasDetail).
			Set("subjects", canonical.Strings(sortedUnique(stringsOf(fault.Field("subjects"))))))
	}
	if frames.Len() > 0 {
		robot = robot.Set("frames", frames)
	}
	if tier := inputs.Field("tier"); tier.Kind() == canonical.KindString {
		robot = robot.Set("tier", tier)
	}

	// ── skill.grants ──
	skill := canonical.Object().Set("grants", canonical.Object().
		Set("tokens", canonical.Strings(sortedUnique(stringsOf(inputs.Field("grants"))))))

	context := canonical.Object().Set("robot", robot).Set("skill", skill)
	if environment := inputs.Field("env"); truthyValue(environment) {
		context = context.Set("env", environment)
	}
	return LiveContext{
		Context:       context,
		Unaddressable: sortedUnique(unaddressable),
		Unreadable:    sortedUnique(unreadable),
	}
}

// shapeLiveContext puts the two omission lists beside the context rather than inside it: a path
// under `robot.` describing the context itself would be one a precondition could read, and a skill
// asserting something about the completeness of its own evaluation context is asserting something
// about the OS rather than about the robot.
func shapeLiveContext(built LiveContext) canonical.Value {
	return canonical.Object().
		Set("context", built.Context).
		Set("unaddressable", canonical.Strings(built.Unaddressable)).
		Set("unreadable", canonical.Strings(built.Unreadable))
}

func liveContextCase(input canonical.Value) (canonical.Value, error) {
	return shapeLiveContext(BuildLiveContext(input)), nil
}
