// §84's motion action space: a body's named motions, and how a policy's motions reach them.
//
// Written from `reference/typescript/embodiment.ts` — `validateMotions`, `canonicalMotion`,
// `actionSpaceOf` and `bindMotion` — and kept beside `embodiment.go` rather than inside it because
// the two halves of that module are already the longest file in this edition.
//
// Three places Go would quietly differ from the reference, each answered once:
//
//   - `typeof value === 'string'` is `Str()`'s second result. A number in the canonical slot
//     declares nothing and is not rendered into a sentence.
//   - `required !== false` and `optional === true` are strict. `truthyValue`, which the channel half
//     of transport uses for its own `optional`, would accept `1` and `"yes"`.
//   - a bound is checked by key **presence**: an explicit null there is a non-finite number JSON
//     cannot spell, not an absent bound.
package contracts

import (
	"math"
	"strconv"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// canonicalMotion is the canonical motion a declaration names — exact, and only a string.
func canonicalMotion(value canonical.Value) (string, bool) {
	text, isString := value.Str()
	if !isString || !vocabulary.Contains(vocabulary.CanonicalMotions, text) {
		return "", false
	}
	return text, true
}

func nameOf(value canonical.Value) string {
	text, _ := value.Field("name").Str()
	return text
}

// ValidateMotions is §84.1's rules over a body's motion list: the first fault in declaration order.
func ValidateMotions(motions canonical.Value) (FrameFault, bool) {
	fault := func(code string, subjects []string, detail string) (FrameFault, bool) {
		return FrameFault{Code: code, Subjects: subjects, Detail: detail}, false
	}

	names := []string{}
	for _, motion := range motions.Items() {
		name := nameOf(motion)
		if name == "" {
			return fault("unnamed", []string{}, "A motion has an empty name, and a name is what a command carries.")
		}
		if containsString(names, name) {
			return fault("duplicate_motion", []string{name},
				"Two motions are named "+name+", so a command naming it is ambiguous.")
		}
		names = append(names, name)

		if declared, isString := motion.Field("motion").Str(); isString {
			if _, known := canonicalMotion(motion.Field("motion")); !known {
				return fault("unknown_motion", []string{name, declared},
					"Motion "+name+" says it is "+declared+", which is not a canonical motion. "+
						"Omit the field for a motion that is none of them.")
			}
		}

		parameters := []string{}
		for _, parameter := range motion.Field("parameters").Items() {
			parameterName := nameOf(parameter)
			if parameterName == "" {
				return fault("unnamed", []string{name}, "Motion "+name+" has a parameter with an empty name.")
			}
			subject := name + "." + parameterName
			if containsString(parameters, parameterName) {
				return fault("duplicate_parameter", []string{subject},
					"Motion "+name+" lists "+parameterName+" twice.")
			}
			parameters = append(parameters, parameterName)

			if _, isBool := parameter.Field("required").Bool(); !isBool {
				return fault("required_undeclared", []string{subject},
					"Parameter "+subject+" does not say whether it is required. It is read as required, "+
						"which refuses plans that omit it; declare true or false.")
			}

			for _, label := range []string{"min", "max"} {
				value, present := parameter.Get(label)
				if !present {
					continue
				}
				if number, isNumber := value.Number(); !isNumber || math.IsInf(number, 0) || math.IsNaN(number) {
					return fault("bound_invalid", []string{subject},
						"Parameter "+subject+" declares "+label+" as a value that is not a finite number.")
				}
			}
			low, hasLow := parameter.Field("min").Number()
			high, hasHigh := parameter.Field("max").Number()
			if hasLow && hasHigh && low > high {
				return fault("bound_invalid", []string{subject},
					"Parameter "+subject+" declares a minimum above its maximum, so no value satisfies it.")
			}
		}
	}
	return FrameFault{}, true
}

func embodimentMotions(input canonical.Value) (canonical.Value, error) {
	return shapeFrameFault(ValidateMotions(input.Field("motions"))), nil
}

// ── Transport over motions, §84.3 ────────────────────────────────────────────

// actionSpace is which negotiation a plan is. See `actionSpaceOf` in the reference for the table.
type actionSpace int

const (
	spaceJoints actionSpace = iota
	spaceMotions
	spaceAmbiguous
	spaceMismatchMotions
	spaceMismatchJoints
)

func actionSpaceOf(source, target canonical.Value) actionSpace {
	emitsMotions := source.Field("motions").Len() > 0
	emitsChannels := source.Field("action_channels").Len() > 0
	takesMotions := target.Field("motions").Len() > 0
	takesJoints := target.Field("control").Field("modes").Len() > 0

	switch {
	case emitsMotions && emitsChannels:
		return spaceAmbiguous
	case emitsMotions && takesMotions:
		return spaceMotions
	case emitsMotions:
		return spaceMismatchMotions
	case emitsChannels && takesMotions && !takesJoints:
		return spaceMismatchJoints
	}
	return spaceJoints
}

func (space actionSpace) refusesBeforeBinding() bool {
	return space == spaceAmbiguous || space == spaceMismatchMotions || space == spaceMismatchJoints
}

func actionSpaceWarning(space actionSpace, slug string) TransportWarning {
	if space == spaceAmbiguous {
		return TransportWarning{
			Code: "action_space_ambiguous",
			Message: "The policy declares both joint channels and named motions. They are two architectures, " +
				"and a package that claims both has not decided which machine it is for.",
		}
	}
	message := "The policy commands joint channels and this body accepts only named motions."
	if space == spaceMismatchMotions {
		message = "The policy commands named motions and this body is not commanded that way."
	}
	return TransportWarning{
		Code: "action_space_mismatch", Message: message, Subjects: []string{slug}, HasSubjects: true,
	}
}

// MotionBinding is how one of a policy's motions reaches this body.
type MotionBinding struct {
	SourceMotion string
	Method       string
	Target       string
	HasTarget    bool
	Missing      []string
	Reason       string
}

// distinctMotions is the body's motions with a repeated name removed, first declaration winning.
func distinctMotions(target canonical.Value) []canonical.Value {
	seen := []string{}
	out := []canonical.Value{}
	for _, motion := range target.Field("motions").Items() {
		name := nameOf(motion)
		if containsString(seen, name) {
			continue
		}
		seen = append(seen, name)
		out = append(out, motion)
	}
	return out
}

func bindMotion(motion, target canonical.Value, warnAbout func(string, string, []string)) MotionBinding {
	body := distinctMotions(target)
	motionName := nameOf(motion)

	// 1. The controller's own name.
	for _, candidate := range body {
		if nameOf(candidate) == motionName {
			return finishMotionBinding(motion, candidate, "direct", "Motion name matches exactly.", warnAbout)
		}
	}

	// 2. The canonical motion, when exactly one body motion is it.
	wanted, hasCanonical := canonicalMotion(motion.Field("motion"))
	same := []canonical.Value{}
	if hasCanonical {
		for _, candidate := range body {
			if offered, known := canonicalMotion(candidate.Field("motion")); known && offered == wanted {
				same = append(same, candidate)
			}
		}
	}
	if len(same) == 1 {
		return finishMotionBinding(motion, same[0], "remap",
			"Mapped by motion "+wanted+" to \""+nameOf(same[0])+"\".", warnAbout)
	}

	// 3. Optional motions may be dropped; required ones are a refusal.
	var why string
	switch {
	case !hasCanonical:
		why = "The motion declares no canonical motion, so it can only match a motion of the same name, and none exists"
	case len(same) > 1:
		why = "This body has " + strconv.Itoa(len(same)) + " " + wanted + " motions and none named \"" +
			motionName + "\", so the vocabulary cannot choose one"
	default:
		why = "This body has no " + wanted + " motion"
	}

	if isTrue(motion.Field("optional")) {
		return MotionBinding{SourceMotion: motionName, Method: "drop", Missing: []string{},
			Reason: why + "; the policy declares the motion optional."}
	}
	return MotionBinding{SourceMotion: motionName, Method: "blocked", Missing: []string{},
		Reason: why + "; the policy requires it."}
}

func finishMotionBinding(
	motion, body canonical.Value, method, reason string, warnAbout func(string, string, []string),
) MotionBinding {
	motionName := nameOf(motion)
	bodyName := nameOf(body)
	supplied := stringsOf(motion.Field("parameters"))

	// `required !== false`: only a boolean false releases a parameter.
	missing := []string{}
	for _, parameter := range body.Field("parameters").Items() {
		required, isBool := parameter.Field("required").Bool()
		released := isBool && !required
		if !released && !containsString(supplied, nameOf(parameter)) {
			missing = append(missing, nameOf(parameter))
		}
	}
	if len(missing) > 0 {
		return MotionBinding{
			SourceMotion: motionName, Method: "blocked", Target: bodyName, HasTarget: true, Missing: missing,
			Reason: "\"" + bodyName + "\" requires a parameter the policy does not supply: " +
				strings.Join(missing, ", ") + ".",
		}
	}

	taken := []string{}
	for _, parameter := range body.Field("parameters").Items() {
		taken = append(taken, nameOf(parameter))
	}
	unused := []string{}
	for at, name := range supplied {
		if containsString(supplied[:at], name) {
			continue
		}
		if !containsString(taken, name) {
			unused = append(unused, name)
		}
	}
	if len(unused) > 0 {
		subjects := make([]string, 0, len(unused))
		for _, name := range unused {
			subjects = append(subjects, motionName+"."+name)
		}
		warnAbout("motion_parameter_unused",
			"\""+bodyName+"\" takes no parameter named "+strings.Join(unused, ", ")+
				", so the policy's value reaches nothing.", subjects)
	}

	return MotionBinding{
		SourceMotion: motionName, Method: method, Target: bodyName, HasTarget: true, Missing: []string{},
		Reason: reason,
	}
}

func deriveMotionVerdict(bindings []MotionBinding) string {
	dropped := false
	for _, binding := range bindings {
		if binding.Method == "blocked" {
			return "refused"
		}
		if binding.Method == "drop" {
			dropped = true
		}
	}
	if dropped {
		return "degraded"
	}
	return "direct"
}

func shapeMotionBindings(bindings []MotionBinding) canonical.Value {
	out := canonical.Array()
	for _, entry := range bindings {
		out = out.Push(canonical.Object().
			Set("source_motion", canonical.String(entry.SourceMotion)).
			Set("method", canonical.String(entry.Method)).
			Set("target", optionalString(entry.Target, entry.HasTarget)).
			Set("missing", canonical.Strings(entry.Missing)).
			Set("reason", canonical.String(entry.Reason)))
	}
	return out
}
