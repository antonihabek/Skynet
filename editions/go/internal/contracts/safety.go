package contracts

import (
	"fmt"
	"math"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The Safety Kernel contract (§7): the resolved limit set, and the per-cycle decision.
//
// Everything here is arithmetic on declared numbers, no clock beyond a tick the caller supplies.
// Two rules decide most of its shape, and both are the reference's:
//
//   - **The enforcement mode is derived from the tier, never supplied.** A caller cannot pass in the
//     mode it would prefer, and the guest tier's `advisory` is the absence of enforcement recorded
//     honestly rather than a weaker setting.
//   - **Every source of ceilings composes by minimum.** The envelope, the transport plan, the
//     operator's profile and a session override are folded together with no precedence order, so no
//     source can raise a limit another lowered.
//
// It is a Guest-ceiling edition (§31.3), and this file does not change that: it computes the verdict
// a kernel would reach. It hosts no kernel and could not enforce one.

// maybeNumber is a field of the reference's type `number | undefined`: absent, or a number. A `null`
// in the input is NaN, by corpus/README.md's bare-number rule, because JSON.stringify is how the
// generator spells a non-finite number.
type maybeNumber struct {
	value   float64
	present bool
}

// bareNumber is a required field of the reference's type `number`: NaN when it is absent or null,
// which is what the reference's arithmetic makes of `undefined` and what the generator's null means.
func bareNumber(object canonical.Value, key string) float64 {
	value, present := object.Get(key)
	number, isNumber := value.Number()
	return js.BareNumber(present, number, isNumber)
}

func numberField(object canonical.Value, key string) maybeNumber {
	value, present := object.Get(key)
	if !present {
		return maybeNumber{}
	}
	number, isNumber := value.Number()
	return maybeNumber{value: js.BareNumber(true, number, isNumber), present: true}
}

// nullableNumber is a maybeNumber as a nullable field on the wire: `x ?? null`.
func (m maybeNumber) nullable() canonical.Value { return optionalNumber(m.value, m.present) }

// optionalObject is a parameter the reference reads through `?.`: an object, or nothing. Absent and
// `null` are one fact here, because `?.` makes them one.
type optionalObject struct {
	value   canonical.Value
	present bool
}

func objectField(parent canonical.Value, key string) optionalObject {
	value, present := parent.Get(key)
	return optionalObject{value: value, present: present && !value.IsNull()}
}

func (o optionalObject) number(key string) maybeNumber {
	if !o.present {
		return maybeNumber{}
	}
	return numberField(o.value, key)
}

// narrowest is the reference's `narrowest`: the minimum of the values that are present and finite.
// An absent input never widens the result, and no inputs at all is null rather than infinity.
func narrowest(values ...maybeNumber) maybeNumber {
	out := maybeNumber{}
	for _, candidate := range values {
		if !candidate.present || !canonical.Finite(candidate.value) {
			continue
		}
		if !out.present || candidate.value < out.value {
			out = maybeNumber{value: candidate.value, present: true}
		}
	}
	return out
}

// tierEnforcement is §31.3's table: what each tier's kernel does with a violation. Derived, never
// supplied (resolveEnforcement in the reference).
var tierEnforcement = map[string]string{
	"guest":     "advisory",
	"host":      "enforcing",
	"certified": "enforcing_redundant",
}

// tierHasIndependentStop is TIER_PROFILES[tier].independent_stop_path: only the certified tier has
// a stop path off the application processor.
func tierHasIndependentStop(tier string) bool { return tier == "certified" }

// SafetyNotice is something an operator must read before this runs.
type SafetyNotice struct {
	Code        string
	Message     string
	Subjects    []string
	HasSubjects bool
}

// UnavailableGuard is a guard specified and inoperable here, with the inputs that are missing.
type UnavailableGuard struct {
	Guard   string
	Missing []string
}

// InertGuard is §80's third state: every declared input present, and nothing producing the
// per-cycle reading the guard acts on. Not `unavailable`, whose remedy is a sensor or a profile;
// this one's remedy is a producer in the operating system, which is a different sentence to a
// different person.
type InertGuard struct {
	Guard   string
	Reading string
}

// Proximity is the speed-and-separation thresholds an operator configured.
type Proximity struct {
	StopDistanceM float64
	SlowDistanceM float64
	SlowFactor    float64
}

// channelCeiling is one per-channel ceiling, in the order the transport plan listed it.
type channelCeiling struct {
	channel string
	value   float64
}

// ResolvedLimits is the single limit set the kernel enforces.
type ResolvedLimits struct {
	Tier          string
	Enforcement   string
	JointVelocity maybeNumber
	JointTorque   maybeNumber
	BaseVelocity  maybeNumber
	// MinControlHz is `transport?.min_control_hz ?? null`.
	MinControlHz        maybeNumber
	MaxDelta            maybeNumber
	PerChannel          []channelCeiling
	Active              []string
	Unavailable         []UnavailableGuard
	Inert               []InertGuard
	Source              canonical.Value
	Notices             []SafetyNotice
	Proximity           *Proximity
	Frames              []Frame
	HasFrames           bool
	Workspace           []WorkspaceLayer
	SelfCollisionMargin maybeNumber
	Grants              []string
}

func (limits ResolvedLimits) active(guard string) bool { return containsString(limits.Active, guard) }

// LimitsParams is ResolveLimitsParams, read out of a document.
type LimitsParams struct {
	Tier      string
	Envelope  optionalObject
	Transport optionalObject
	Profile   optionalObject
	Session   optionalObject
	Frames    []Frame
	HasFrames bool
	Available canonical.Value
}

// ReadLimitsParams reads the parameters a case supplies, flattening the frame graph it carries — the
// graph rather than pre-flattened frames, because flattening is itself part of what the corpus checks.
func ReadLimitsParams(input canonical.Value) (LimitsParams, error) {
	tier, _ := input.Field("tier").Str()
	if _, known := tierEnforcement[tier]; !known {
		// The reference indexes TIER_PROFILES with it and would throw; an edition inventing a tier's
		// enforcement would be the one thing §31.3's rule 2 says an edition may not do.
		return LimitsParams{}, fmt.Errorf("the tier %q is not one of %v", tier, vocabulary.OSTiers)
	}
	params := LimitsParams{
		Tier:      tier,
		Envelope:  objectField(input, "envelope"),
		Transport: objectField(input, "transport"),
		Profile:   objectField(input, "profile"),
		Session:   objectField(input, "session"),
		Available: input.Field("available"),
	}
	if graph, present := presentNonNull(input, "frames"); present {
		frames, err := FlattenDocument(graph)
		if err != nil {
			return LimitsParams{}, err
		}
		params.Frames, params.HasFrames = frames, true
	}
	return params, nil
}

// readWorkspace reads an operator's layered volumes.
func readWorkspace(value canonical.Value) []WorkspaceLayer {
	layers := []WorkspaceLayer{}
	for _, entry := range value.Items() {
		name, _ := entry.Field("name").Str()
		layer := WorkspaceLayer{Name: name, Volumes: []Volume{}}
		for _, volume := range entry.Field("volumes").Items() {
			frame, _ := volume.Field("frame").Str()
			shape := volume.Field("shape")
			kind, _ := shape.Field("kind").Str()
			layer.Volumes = append(layer.Volumes, Volume{Frame: frame, Shape: VolumeShape{
				Kind:    kind,
				Min:     vec3Of(shape.Field("min")),
				Max:     vec3Of(shape.Field("max")),
				Centre:  vec3Of(shape.Field("centre")),
				RadiusM: bareNumber(shape, "radius_m"),
			}})
		}
		layers = append(layers, layer)
	}
	return layers
}

// ResolveLimits composes every source of ceilings into the set the kernel enforces.
func ResolveLimits(params LimitsParams) ResolvedLimits {
	envelope, transport, profile, session := params.Envelope, params.Transport, params.Profile, params.Session
	enforcement := tierEnforcement[params.Tier]
	notices := []SafetyNotice{}
	notice := func(code, message string) {
		notices = append(notices, SafetyNotice{Code: code, Message: message})
	}
	noticeAbout := func(code, message string, subjects []string) {
		notices = append(notices, SafetyNotice{Code: code, Message: message, Subjects: subjects, HasSubjects: true})
	}

	if enforcement == "advisory" {
		notice("advisory_only",
			"At the guest tier Skynet OS does not own the control loop. Violations are detected and "+
				"recorded; commands still reach the robot unchanged. This is not a safety function.")
	}
	if !tierHasIndependentStop(params.Tier) {
		notice("no_independent_stop",
			"There is no stop path independent of the application processor. If the processor hangs, "+
				"nothing here stops the robot.")
	}

	if !envelope.present {
		notice("no_envelope",
			"No safety envelope has been declared for this robot. Only per-joint ratings from the "+
				"body description are available, and whole-body ceilings are unenforced.")
	} else if !truthyValue(envelope.value.Field("source")) {
		notice("unsourced_limits", "The declared limits cite no datasheet. A safety limit without provenance is a rumour.")
	}

	// Widening attempts are recorded, then discarded by the minimum. Silently ignoring one would leave
	// an operator believing a limit they set is in force.
	widened := []string{}
	checkWiden := func(label string, requested, ceiling maybeNumber) {
		if requested.present && ceiling.present && requested.value > ceiling.value {
			widened = append(widened, label)
		}
	}
	for _, field := range []string{"joint_velocity_max_rad_s", "joint_torque_max_nm", "base_velocity_max_m_s"} {
		checkWiden(field, profile.number(field), envelope.number(field))
	}
	if len(widened) > 0 {
		name, present := profile.value.Get("name")
		noticeAbout("profile_widened",
			"The operator profile \""+js.ToString(name, present)+"\" asks to raise limits above the declared "+
				"envelope. Those values are ignored.", widened)
	}

	// §38.2's session layer, checked against what the layer above it allows — everything already
	// composed — rather than against the envelope alone. A session asking for 3 rad/s under a
	// profile allowing 1 has widened the thing actually in force.
	sessionWidened := []string{}
	if session.present {
		for _, field := range []string{"joint_velocity_max_rad_s", "joint_torque_max_nm", "base_velocity_max_m_s"} {
			above := narrowest(envelope.number(field), transport.number(field), profile.number(field))
			requested := session.number(field)
			if requested.present && above.present && requested.value > above.value {
				sessionWidened = append(sessionWidened, field)
			}
		}
		requested, ceiling := session.number("max_delta_per_cycle"), profile.number("max_delta_per_cycle")
		if requested.present && ceiling.present && requested.value > ceiling.value {
			sessionWidened = append(sessionWidened, "max_delta_per_cycle")
		}
	}
	if len(sessionWidened) > 0 {
		noticeAbout("session_widened",
			"The session override asks to raise limits above what is already in force. Those values "+
				"are ignored; the narrower ceiling still applies.", sessionWidened)
	}

	// The session joins the same minimum as every other layer, which is what makes "it only ever
	// narrows" a property of the arithmetic rather than a rule somebody has to remember.
	ceiling := func(field string) maybeNumber {
		return narrowest(envelope.number(field), transport.number(field), profile.number(field), session.number(field))
	}
	jointVelocity := ceiling("joint_velocity_max_rad_s")
	jointTorque := ceiling("joint_torque_max_nm")
	baseVelocity := ceiling("base_velocity_max_m_s")

	maxDelta := narrowest(profile.number("max_delta_per_cycle"), session.number("max_delta_per_cycle"))
	if !maxDelta.present {
		notice("no_rate_limit",
			"No jerk ceiling is configured, so a single-cycle jump from zero to the clamped maximum is "+
				"permitted. Set max_delta_per_cycle before running a policy near people.")
	}

	if session.present {
		name, present := session.value.Get("name")
		sessionName := js.ToString(name, present)
		noticeAbout("session_override",
			"A session override \""+sessionName+"\" is narrowing the limits in force. It is a temporary "+
				"setting rather than this robot's configuration — clear it when the work is done.",
			[]string{sessionName})
	}

	workspace := []WorkspaceLayer{}
	if profile.present {
		if layers, present := presentNonNull(profile.value, "workspace"); present {
			workspace = readWorkspace(layers)
		}
	}
	var proximity *Proximity
	if profile.present && truthyValue(profile.value.Field("proximity")) {
		configured := profile.value.Field("proximity")
		proximity = &Proximity{
			StopDistanceM: bareNumber(configured, "stop_distance_m"),
			SlowDistanceM: bareNumber(configured, "slow_distance_m"),
			SlowFactor:    bareNumber(configured, "slow_factor"),
		}
	}

	active, unavailable, inert := partitionGuards(params, maxDelta, workspace)

	// Emitted only when the guard it is about will actually run: a robot with no collision geometry is
	// already being told the guard is unavailable. `=== undefined`, so a margin written as `null` is a
	// margin someone set, and no notice.
	marginDeclared := profile.present && profile.value.Has("self_collision_margin_m")
	if containsString(active, "self_collision") && !marginDeclared {
		notice("no_collision_margin",
			"No self-collision margin is configured, so two of this robot's own bodies must actually "+
				"meet before the guard holds. Set self_collision_margin_m to stop the command that would "+
				"bring them together.")
	}
	for _, entry := range unavailable {
		noticeAbout("guard_unavailable", "The "+entry.Guard+" guard cannot run on this robot.", entry.Missing)
	}

	perChannel := []channelCeiling{}
	if transport.present {
		for _, member := range transport.value.Field("per_channel").Members() {
			number, isNumber := member.Value.Number()
			perChannel = append(perChannel, channelCeiling{channel: member.Key, value: js.BareNumber(true, number, isNumber)})
		}
	}

	// `envelope?.source ?? transport?.source ?? null`: `??` passes an empty string through, which is
	// why this is not a truthiness test.
	source := canonical.Null()
	if value, present := presentNonNull(envelope.value, "source"); envelope.present && present {
		source = value
	} else if value, present := presentNonNull(transport.value, "source"); transport.present && present {
		source = value
	}

	selfCollisionMargin := maybeNumber{}
	if profile.present {
		if _, present := presentNonNull(profile.value, "self_collision_margin_m"); present {
			selfCollisionMargin = profile.number("self_collision_margin_m")
		}
	}
	minControlHz := maybeNumber{}
	if transport.present {
		if _, present := presentNonNull(transport.value, "min_control_hz"); present {
			minControlHz = transport.number("min_control_hz")
		}
	}

	return ResolvedLimits{
		Tier:                params.Tier,
		Enforcement:         enforcement,
		JointVelocity:       jointVelocity,
		JointTorque:         jointTorque,
		BaseVelocity:        baseVelocity,
		MinControlHz:        minControlHz,
		MaxDelta:            maxDelta,
		PerChannel:          perChannel,
		Proximity:           proximity,
		Frames:              params.Frames,
		HasFrames:           params.HasFrames,
		Workspace:           workspace,
		SelfCollisionMargin: selfCollisionMargin,
		Grants:              []string{},
		Active:              active,
		Unavailable:         unavailable,
		Inert:               inert,
		Source:              source,
		Notices:             notices,
	}
}

// partitionGuards decides, for each of the eight guards in evaluation order, whether it runs, cannot
// run for want of a named input, or is inert for want of a producer.
func partitionGuards(params LimitsParams, maxDelta maybeNumber, workspace []WorkspaceLayer) ([]string, []UnavailableGuard, []InertGuard) {
	active := []string{}
	unavailable := []UnavailableGuard{}
	inert := []InertGuard{}
	envelope, profile, available := params.Envelope, params.Profile, params.Available
	frames, hasFrames := params.Frames, params.HasFrames

	missingFor := func(guard string) []string {
		switch guard {
		case "watchdog":
			// `envelope?.watchdog_ms ? … ` — truthy, so a declared zero is no watchdog.
			if envelope.present && truthyValue(envelope.value.Field("watchdog_ms")) {
				return nil
			}
			return []string{"envelope.watchdog_ms"}
		case "envelope", "mode_gate":
			return nil
		case "rate_limit":
			if !maxDelta.present {
				return []string{"profile.max_delta_per_cycle"}
			}
			return nil
		case "workspace":
			// §44.5: a body with no frame graph, or whose graph does not reach the frame the operator
			// constrained, reports the guard unavailable with the missing frame named — never evaluated
			// against a frame it guessed.
			missing := []string{}
			volumes := 0
			for _, layer := range workspace {
				volumes += len(layer.Volumes)
			}
			if volumes == 0 {
				missing = append(missing, "profile.workspace")
			}
			if !hasFrames {
				missing = append(missing, "embodiment.frames")
			} else {
				for _, layer := range workspace {
					for _, volume := range layer.Volumes {
						index := -1
						for at, frame := range frames {
							if frame.Name == volume.Frame {
								index = at
								break
							}
						}
						if (index < 0 || !staticallyResolvable(frames, index)) && !containsString(missing, volume.Frame) {
							missing = append(missing, volume.Frame)
						}
					}
				}
			}
			return missing
		case "self_collision":
			if !hasFrames {
				return []string{"embodiment.frames"}
			}
			// Two, not one: a single collision body has nothing to collide with, and a guard reported
			// active with no pair to test is a guard that passes by not looking.
			if collisionBodies(frames) < 2 {
				return []string{"embodiment.frames[].collision, on two or more frames"}
			}
			return nil
		case "balance":
			// Not applicable rather than missing on a wheeled base; the list says which, so nobody
			// reads "balance guard off" as a defect.
			if !truthyValue(available.Field("legged")) {
				return []string{"a legged base — not applicable to this body"}
			}
			if truthyValue(available.Field("imu")) {
				return nil
			}
			return []string{"an IMU"}
		case "proximity":
			if !profile.present || !truthyValue(profile.value.Field("proximity")) {
				return []string{"profile.proximity"}
			}
			if truthyValue(available.Field("people_detection")) {
				return nil
			}
			return []string{"a sensor that detects people"}
		}
		return nil
	}

	for _, guard := range vocabulary.Guards {
		if missing := missingFor(guard); len(missing) > 0 {
			unavailable = append(unavailable, UnavailableGuard{Guard: guard, Missing: missing})
			continue
		}
		// Checked after `missing`, and the order is the point: a guard with no sensor is unavailable,
		// not inert. `proximity` is the only guard whose reading can be absent while every declared
		// input is present — the other seven act on the command, the previous command, the clock or
		// the frame graph, which every cycle carries. `=== true`: a caller that has not said has not
		// established it, and a guard is never given the benefit of the doubt.
		if guard == "proximity" && !isTrue(available.Field("proximity_reading")) {
			inert = append(inert, InertGuard{Guard: guard, Reading: "cycle.nearest_person_m"})
			continue
		}
		active = append(active, guard)
	}
	return active, unavailable, inert
}

// ── the per-cycle decision ──────────────────────────────────────────────────

// channelValue is one entry of a command, in the order the policy's document listed it. An ordered
// slice rather than a map, because `clamped` is an array whose order follows the iteration of the
// command — and Go would iterate a map in a different order on every run.
type channelValue struct {
	channel string
	value   float64
}

func readChannels(value canonical.Value) []channelValue {
	out := []channelValue{}
	for _, member := range value.Members() {
		number, isNumber := member.Value.Number()
		out = append(out, channelValue{channel: member.Key, value: js.BareNumber(true, number, isNumber)})
	}
	return out
}

func channelsObject(channels []channelValue) canonical.Value {
	out := canonical.Object()
	for _, entry := range channels {
		out = out.Set(entry.channel, canonical.Number(entry.value))
	}
	return out
}

// CommandCycle is one control cycle's input.
type CommandCycle struct {
	NowMS     float64
	CommandMS float64
	Sequence  float64
	Desired   []channelValue
	// DesiredDocument is the desired command as the document wrote it, forwarded unchanged at the
	// guest tier: `{ ...cycle.desired }`, which is the input's values, not this edition's reading of
	// them.
	DesiredDocument canonical.Value
	Previous        []channelValue
	HasPrevious     bool
	Capability      canonical.Value
	HasCapability   bool
	NearestPersonM  maybeNumber
}

// ReadCommandCycle reads a cycle a case supplies.
func ReadCommandCycle(value canonical.Value) CommandCycle {
	cycle := CommandCycle{
		NowMS:           bareNumber(value, "now_ms"),
		CommandMS:       bareNumber(value, "command_ms"),
		Sequence:        bareNumber(value, "sequence"),
		Desired:         readChannels(value.Field("desired")),
		DesiredDocument: value.Field("desired"),
		NearestPersonM:  numberField(value, "nearest_person_m"),
	}
	if previous, present := presentNonNull(value, "previous"); present {
		cycle.Previous, cycle.HasPrevious = readChannels(previous), true
	}
	cycle.Capability, cycle.HasCapability = value.Get("capability")
	return cycle
}

// PoseTrip is one pose-guard finding. Margin is negative for a violation and absent when the frame
// had no pose this cycle — which is a trip in its own right, because a guard that cannot establish
// where a constrained frame is has not shown the command safe.
type PoseTrip struct {
	Guard  string
	Frame  string
	Other  string
	HasOth bool
	Margin maybeNumber
}

// PoseOutcome is what the two pose guards found and what they were able to look at. Present on
// every verdict, empty or not: omitting it when nothing tripped would make an edition that never ran
// the pose guards byte-identical to one that ran them and found nothing.
type PoseOutcome struct {
	Evaluated  bool
	Lookahead  bool
	Truncated  bool
	Unresolved []string
	Trips      []PoseTrip
}

func emptyPose() PoseOutcome {
	return PoseOutcome{Unresolved: []string{}, Trips: []PoseTrip{}}
}

// clampRecord is one channel whose value a guard reduced.
type clampRecord struct {
	channel  string
	from, to float64
}

// CycleVerdict is what the kernel will send, and why.
type CycleVerdict struct {
	// Command is nil for `{}`, under a hold or a stop.
	Command         []channelValue
	CommandDocument canonical.Value
	HasDocument     bool
	Response        string
	HasResponse     bool
	Tripped         []string
	Clamped         []clampRecord
	Advisory        bool
	Pose            PoseOutcome
}

// maxPoseTrips is how many pose findings a verdict carries; past it the list is truncated and says so.
const maxPoseTrips = 8

func responseRank(response string) int {
	for at, candidate := range vocabulary.FaultResponses {
		if candidate == response {
			return at
		}
	}
	return -1
}

// stronger is the stronger of two graduated responses, weakest to strongest per IEC 60204-1.
func stronger(current string, hasCurrent bool, response string) string {
	if !hasCurrent || responseRank(current) < responseRank(response) {
		return response
	}
	return current
}

// jsSign is Math.sign: NaN for NaN, and a signed zero for a signed zero.
func jsSign(value float64) float64 {
	switch {
	case math.IsNaN(value):
		return value
	case value > 0:
		return 1
	case value < 0:
		return -1
	}
	return value
}

func ceilingFor(channel string, limits ResolvedLimits) maybeNumber {
	for _, entry := range limits.PerChannel {
		if entry.channel == channel {
			return maybeNumber{value: entry.value, present: true}
		}
	}
	if channel == "base" {
		return limits.BaseVelocity
	}
	return maybeNumber{}
}

func appendOnce(values []string, value string) []string {
	if containsString(values, value) {
		return values
	}
	return append(values, value)
}

// EvaluateCycle evaluates one control cycle: the watchdog, magnitude clamping, rate limiting, the two
// pose guards, proximity and the mode gate, in the order §7.1 fixes. Order is part of the
// specification — clamping creates step changes of its own, and the rate limiter is what turns them
// into a ramp.
func EvaluateCycle(
	cycle CommandCycle, limits ResolvedLimits, envelope optionalObject,
	lastSequence maybeNumber, bindings []JointBinding,
) CycleVerdict {
	tripped := []string{}
	clamped := []clampRecord{}
	advisory := limits.Enforcement == "advisory"
	forwarded := func(computed []channelValue, withheld bool) CycleVerdict {
		verdict := CycleVerdict{Tripped: tripped, Clamped: clamped, Advisory: advisory}
		switch {
		case advisory:
			// In advisory mode the desired command is what reaches the robot. Returning the clamped
			// values would make a guest-tier deployment look enforced in its own logs.
			verdict.CommandDocument, verdict.HasDocument = cycle.DesiredDocument, true
		case withheld:
			verdict.Command = []channelValue{}
		default:
			verdict.Command = computed
		}
		return verdict
	}

	// The strongest response any guard demanded this cycle — not the strongest any tripped guard could
	// demand. `proximity` between its two distances scales and forwards, and a verdict read off the
	// guard's row would say "stopped" about a robot that was not.
	demanded, hasDemanded := "", false
	demand := func(response string) {
		demanded = stronger(demanded, hasDemanded, response)
		hasDemanded = true
	}

	// 1. The watchdog. A stale command is discarded, never clamped and forwarded — clamping it would
	//    keep a dead producer's last intention alive at a legal magnitude.
	watchdog := envelope.number("watchdog_ms")
	stale := watchdog.present && cycle.NowMS-cycle.CommandMS > watchdog.value
	stalled := lastSequence.present && cycle.Sequence <= lastSequence.value
	if limits.active("watchdog") && (stale || stalled) {
		tripped = append(tripped, "watchdog")
		verdict := forwarded(nil, true)
		verdict.Response, verdict.HasResponse = "stop_cat2", true
		verdict.Pose = emptyPose()
		return verdict
	}

	// 2. Magnitude clamping against the resolved ceilings.
	command := make([]channelValue, 0, len(cycle.Desired))
	for _, entry := range cycle.Desired {
		next := entry.value
		if ceiling := ceilingFor(entry.channel, limits); ceiling.present && math.Abs(entry.value) > ceiling.value {
			next = float64(jsSign(entry.value) * ceiling.value)
			tripped = appendOnce(tripped, "envelope")
			demand("clamp")
			clamped = append(clamped, clampRecord{channel: entry.channel, from: entry.value, to: next})
		}
		command = setChannel(command, entry.channel, next)
	}

	// 3. Rate limiting, after clamping, because clamping creates step changes of its own.
	if limits.MaxDelta.present && cycle.HasPrevious {
		for at, entry := range command {
			previous, found := channelIn(cycle.Previous, entry.channel)
			if !found {
				continue
			}
			ceiling := ceilingFor(entry.channel, limits)
			maxStep := float64(ceiling.value * limits.MaxDelta.value)
			if !ceiling.present {
				maxStep = float64(math.Abs(previous) * limits.MaxDelta.value)
			}
			delta := entry.value - previous
			if math.Abs(delta) > maxStep && maxStep > 0 {
				next := previous + float64(jsSign(delta)*maxStep)
				tripped = appendOnce(tripped, "rate_limit")
				demand("clamp")
				clamped = append(clamped, clampRecord{channel: entry.channel, from: entry.value, to: next})
				command[at].value = next
			}
		}
	}

	// 4 and 5. The pose guards, on the configuration the command above would produce — after clamping
	//    and rate limiting, because the pose that matters is the one the robot will be commanded into.
	pose := emptyPose()
	if (limits.active("workspace") || limits.active("self_collision")) && limits.HasFrames {
		frames := limits.Frames
		solved := Solve(frames, bindings, channelsObject(command))
		pose.Evaluated = true
		pose.Lookahead = solved.Lookahead
		for _, at := range solved.Unresolved {
			pose.Unresolved = append(pose.Unresolved, frames[at].Name)
		}
		if limits.active("workspace") {
			checkWorkspace(frames, solved.Poses, limits.Workspace, &pose)
		}
		if limits.active("self_collision") {
			marginM := 0.0
			if limits.SelfCollisionMargin.present {
				marginM = limits.SelfCollisionMargin.value
			}
			checkSelfCollision(frames, solved.Poses, marginM, &pose)
		}
		if len(pose.Trips) > maxPoseTrips {
			pose.Trips = pose.Trips[:maxPoseTrips]
			pose.Truncated = true
		}
		for _, trip := range pose.Trips {
			tripped = appendOnce(tripped, trip.Guard)
			demand("hold")
		}
	}

	// 7. Proximity: speed and separation monitoring, applied to the command the magnitude and rate
	//    guards produced, so a later stage can only narrow.
	if limits.active("proximity") && cycle.NearestPersonM.present && limits.Proximity != nil {
		separation, proximity := cycle.NearestPersonM.value, limits.Proximity
		if separation <= proximity.StopDistanceM {
			tripped = appendOnce(tripped, "proximity")
			verdict := forwarded(nil, true)
			verdict.Response, verdict.HasResponse = "stop_cat1", true
			verdict.Pose = pose
			return verdict
		}
		if separation <= proximity.SlowDistanceM && proximity.SlowFactor < 1 {
			tripped = appendOnce(tripped, "proximity")
			demand("clamp")
			for at, entry := range command {
				next := float64(entry.value * proximity.SlowFactor)
				if next != entry.value {
					clamped = append(clamped, clampRecord{channel: entry.channel, from: entry.value, to: next})
					command[at].value = next
				}
			}
		}
	}

	// 8. The mode gate, every cycle rather than at start, so a revocation takes effect on the next
	//    command (§8.2). A capability the cycle does not name is not evaluated: absent is not "no
	//    capability".
	if limits.active("mode_gate") && cycle.HasCapability {
		capability, isString := cycle.Capability.Str()
		if !isString || !containsString(limits.Grants, capability) {
			tripped = appendOnce(tripped, "mode_gate")
			demand("hold")
		}
	}

	// A hold means the command does not go out, stated once here rather than at each guard.
	withheld := hasDemanded && responseRank(demanded) >= responseRank("hold")
	verdict := forwarded(command, withheld)
	verdict.Response, verdict.HasResponse = demanded, hasDemanded
	verdict.Pose = pose
	return verdict
}

// setChannel is `command[channel] = value` on an ordered command: a repeated key keeps its first
// position, as an object's does.
func setChannel(command []channelValue, channel string, value float64) []channelValue {
	for at := range command {
		if command[at].channel == channel {
			command[at].value = value
			return command
		}
	}
	return append(command, channelValue{channel: channel, value: value})
}

func channelIn(channels []channelValue, channel string) (float64, bool) {
	for _, entry := range channels {
		if entry.channel == channel {
			return entry.value, true
		}
	}
	return 0, false
}

// checkWorkspace is §7.1's fourth guard: every constrained frame inside its operator-defined volumes.
// Layers compose by conjunction — the frame must satisfy every layer that names it, and the reported
// margin is the worst of them — while the volumes within one layer are alternatives.
func checkWorkspace(frames []Frame, poses []*Transform, layers []WorkspaceLayer, pose *PoseOutcome) {
	for index, frame := range frames {
		constrained := false
		worst := maybeNumber{}
		for _, layer := range layers {
			best := maybeNumber{}
			for _, volume := range layer.Volumes {
				if volume.Frame != frame.Name {
					continue
				}
				constrained = true
				if solved := poses[index]; solved != nil {
					value := margin(volume.Shape, solved.Translation)
					if !best.present || value > best.value {
						best = maybeNumber{value: value, present: true}
					}
				}
			}
			if best.present && (!worst.present || best.value < worst.value) {
				worst = best
			}
		}

		if !constrained {
			continue
		}
		if poses[index] == nil {
			// Constrained and unlocatable. Not a pass: the guard has not shown the command safe.
			pose.Trips = append(pose.Trips, PoseTrip{Guard: "workspace", Frame: frame.Name})
			continue
		}
		if worst.present && worst.value < 0 {
			pose.Trips = append(pose.Trips, PoseTrip{Guard: "workspace", Frame: frame.Name, Margin: worst})
		}
	}
}

// checkSelfCollision is §7.1's fifth guard: no two collision bodies closer than the operator's
// margin. Pairs are visited in index order so the list is the same on every edition, and adjacent
// frames and declared exceptions are skipped before any arithmetic.
func checkSelfCollision(frames []Frame, poses []*Transform, marginM float64, pose *PoseOutcome) {
	clearance := 0.0
	if marginM > 0 {
		clearance = marginM
	}
	for i := range frames {
		first := frames[i].Collision
		if first == nil {
			continue
		}
		for j := i + 1; j < len(frames); j++ {
			second := frames[j].Collision
			if second == nil || adjacent(frames, i, j) || ignores(frames, i, j) {
				continue
			}
			poseI, poseJ := poses[i], poses[j]
			if poseI == nil || poseJ == nil {
				// Reported against the pair rather than skipped: a collision test that quietly excludes
				// a limb is a test that passes because it did not look.
				pose.Trips = append(pose.Trips, PoseTrip{
					Guard: "self_collision", Frame: frames[i].Name, Other: frames[j].Name, HasOth: true,
				})
				continue
			}
			a1, b1 := capsuleInWorld(*poseI, *first)
			a2, b2 := capsuleInWorld(*poseJ, *second)
			squared := segmentDistanceSquared(a1, b1, a2, b2)
			threshold := first.RadiusM + second.RadiusM + clearance
			// Decided in squared distance, so no square root can move a verdict; the root is taken only
			// for the number the report carries.
			if squared < float64(threshold*threshold) {
				pose.Trips = append(pose.Trips, PoseTrip{
					Guard: "self_collision", Frame: frames[i].Name, Other: frames[j].Name, HasOth: true,
					Margin: maybeNumber{value: distance(squared) - first.RadiusM - second.RadiusM - clearance, present: true},
				})
			}
		}
	}
}

// ── the wire ────────────────────────────────────────────────────────────────

func shapeNotices(notices []SafetyNotice) canonical.Value {
	out := canonical.Array()
	for _, entry := range notices {
		out = out.Push(canonical.Object().
			Set("code", canonical.String(entry.Code)).
			Set("message", canonical.String(entry.Message)).
			SetIf("subjects", canonical.Strings(entry.Subjects), entry.HasSubjects))
	}
	return out
}

func shapePerChannel(ceilings []channelCeiling) canonical.Value {
	out := canonical.Object()
	for _, entry := range ceilings {
		out = out.Set(entry.channel, canonical.Number(entry.value))
	}
	return out
}

func shapeLimits(limits ResolvedLimits) canonical.Value {
	unavailable := canonical.Array()
	for _, entry := range limits.Unavailable {
		unavailable = unavailable.Push(canonical.Object().
			Set("guard", canonical.String(entry.Guard)).
			Set("missing", canonical.Strings(entry.Missing)))
	}
	inert := canonical.Array()
	for _, entry := range limits.Inert {
		inert = inert.Push(canonical.Object().
			Set("guard", canonical.String(entry.Guard)).
			Set("reading", canonical.String(entry.Reading)))
	}
	return canonical.Object().
		Set("tier", canonical.String(limits.Tier)).
		Set("enforcement", canonical.String(limits.Enforcement)).
		Set("joint_velocity_max_rad_s", limits.JointVelocity.nullable()).
		Set("joint_torque_max_nm", limits.JointTorque.nullable()).
		Set("base_velocity_max_m_s", limits.BaseVelocity.nullable()).
		Set("min_control_hz", limits.MinControlHz.nullable()).
		Set("max_delta_per_cycle", limits.MaxDelta.nullable()).
		Set("self_collision_margin_m", limits.SelfCollisionMargin.nullable()).
		Set("per_channel", shapePerChannel(limits.PerChannel)).
		Set("active_guards", canonical.Strings(limits.Active)).
		Set("unavailable_guards", unavailable).
		Set("inert_guards", inert).
		Set("source", limits.Source).
		Set("notices", shapeNotices(limits.Notices))
}

func shapePose(pose PoseOutcome) canonical.Value {
	trips := canonical.Array()
	for _, trip := range pose.Trips {
		trips = trips.Push(canonical.Object().
			Set("guard", canonical.String(trip.Guard)).
			Set("frame", canonical.String(trip.Frame)).
			Set("other", optionalString(trip.Other, trip.HasOth)).
			Set("margin_m", trip.Margin.nullable()))
	}
	return canonical.Object().
		Set("evaluated", canonical.Bool(pose.Evaluated)).
		Set("lookahead", canonical.Bool(pose.Lookahead)).
		Set("truncated", canonical.Bool(pose.Truncated)).
		Set("unresolved", canonical.Strings(pose.Unresolved)).
		Set("trips", trips)
}

func shapeVerdict(verdict CycleVerdict) canonical.Value {
	command := channelsObject(verdict.Command)
	if verdict.HasDocument {
		// A fresh object carrying the document's own members, rather than the document itself: a
		// Value shares its storage, and nothing downstream may be able to write into a case's input.
		command = canonical.Object()
		for _, member := range verdict.CommandDocument.Members() {
			command = command.Set(member.Key, member.Value)
		}
	}
	clamped := canonical.Array()
	for _, entry := range verdict.Clamped {
		clamped = clamped.Push(canonical.Object().
			Set("channel", canonical.String(entry.channel)).
			Set("from", canonical.Number(entry.from)).
			Set("to", canonical.Number(entry.to)))
	}
	return canonical.Object().
		Set("command", command).
		Set("response", optionalString(verdict.Response, verdict.HasResponse)).
		Set("tripped", canonical.Strings(verdict.Tripped)).
		Set("clamped", clamped).
		Set("advisory", canonical.Bool(verdict.Advisory)).
		Set("pose", shapePose(verdict.Pose))
}

// ── the corpus ──────────────────────────────────────────────────────────────

func limitsResolution(input canonical.Value) (canonical.Value, error) {
	params, err := ReadLimitsParams(input)
	if err != nil {
		return canonical.Null(), err
	}
	return shapeLimits(ResolveLimits(params)), nil
}

// cycleEvaluate answers every `cycle/*` file and `fuzz/cycle`: the limits a case describes, resolved,
// with the grants attached, and one cycle evaluated against them.
func cycleEvaluate(input canonical.Value) (canonical.Value, error) {
	limitsInput := input.Field("limits")
	params, err := ReadLimitsParams(limitsInput)
	if err != nil {
		return canonical.Null(), err
	}
	limits := ResolveLimits(params)
	limits.Grants = stringsOf(limitsInput.Field("grants"))

	// `last_sequence: null` is how the generator writes "no previous sequence" — the reference's
	// `undefined` — so null here is absence, not NaN.
	lastSequence := maybeNumber{}
	if value, present := presentNonNull(input, "last_sequence"); present {
		number, isNumber := value.Number()
		lastSequence = maybeNumber{value: js.BareNumber(true, number, isNumber), present: true}
	}

	verdict := EvaluateCycle(
		ReadCommandCycle(input.Field("cycle")), limits, params.Envelope,
		lastSequence, ReadJointBindings(input.Field("joints")),
	)
	return shapeVerdict(verdict), nil
}
