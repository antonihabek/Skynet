package contracts

import (
	"regexp"
	"strconv"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// ParsedRole is a canonical role, taken apart (§5.1).
type ParsedRole struct {
	Chain string
	Link  string
	// Axis is empty when the role names none. Distinguished from a declared axis by HasAxis
	// rather than by the empty string, because the wire spells the absence `null`.
	Axis    string
	HasAxis bool
	// Index is the ordinal for repeated links — `base.wheel#2`, `right_hand.finger#1`.
	Index    float64
	HasIndex bool
}

// rolePattern is the reference's, with one deliberate respelling.
//
// The reference writes `(?:#(\d+))?`. In JavaScript, without the `u` flag, `\d` is exactly [0-9] —
// and in a great many other regex dialects it is not. The Python edition's `\d` matched
// Arabic-Indic digits, so `base.wheel#١` parsed as ordinal 1 where the reference refuses it; the
// corpus now holds that exact string. Go's `\d` *is* [0-9], so this would be correct either way,
// and it is spelt out anyway: the next reader should be able to see which of the two was meant
// without knowing Go's answer, and `cmd/selfcheck` refuses the shorthand in this package.
//
// `$` is the other half of the same lesson. Python's `$` matches before a final newline, so
// `list.0\n` indexed element 0. Go's `$` is end-of-text unless `(?m)` is set, which matches
// JavaScript's without the `m` flag.
var rolePattern = regexp.MustCompile(`^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#([0-9]+))?$`)

// ParseRole parses a canonical role, reporting whether it is one.
//
// The reference returns null rather than throwing, and the caller treats null as "this joint can
// only be matched by exact name". That is the extensibility rule that keeps the vocabulary honest:
// an unrecognised role never *silently* maps to something plausible. A body the vocabulary does not
// describe is still usable — it just cannot participate in cross-body remapping, which is the
// truthful outcome, because nobody has established what its joints mean.
func ParseRole(role string) (ParsedRole, bool) {
	// The reference's `if (!role) return null` — falsy, which for a string is exactly the empty
	// one. A role of whitespace is not caught here; it fails the pattern after trimming.
	if !js.Truthy(role) {
		return ParsedRole{}, false
	}
	match := rolePattern.FindStringSubmatch(js.ToLower(js.Trim(role)))
	if match == nil {
		return ParsedRole{}, false
	}
	chain, link, axis, index := match[1], match[2], match[3], match[4]
	if !vocabulary.Contains(vocabulary.CanonicalChains, chain) {
		return ParsedRole{}, false
	}
	// `if (axis && ...)`: an axis that did not participate is the empty string, which is falsy, and
	// the check is skipped rather than failing on a chain that has no axis.
	if js.Truthy(axis) && !vocabulary.Contains(vocabulary.CanonicalAxes, axis) {
		return ParsedRole{}, false
	}
	parsed := ParsedRole{Chain: chain, Link: link, Axis: axis, HasAxis: js.Truthy(axis)}
	if js.Truthy(index) {
		// `[0-9]+` cannot fail to parse, and a value too large for a float64 loses precision
		// exactly as Number() does. The error is checked rather than dropped because R-15 does not
		// have an exception for calls that cannot fail today.
		value, err := strconv.ParseFloat(index, 64)
		if err != nil {
			return ParsedRole{}, false
		}
		parsed.Index, parsed.HasIndex = value, true
	}
	return parsed, true
}

// RoleKey is the canonical string form, so two parsed roles can be compared as keys.
func RoleKey(parsed ParsedRole) string {
	key := parsed.Chain + "." + parsed.Link
	if parsed.HasAxis {
		key += "." + parsed.Axis
	}
	if parsed.HasIndex {
		// String(number), not the canonical writer: the reference interpolates the ordinal into
		// a template literal, and the two spell a large one differently. See js.NumberToString.
		key += "#" + js.NumberToString(parsed.Index)
	}
	return key
}

// rolesVocabulary answers `roles/vocabulary`: a role string, parsed, or null.
func rolesVocabulary(input canonical.Value) (canonical.Value, error) {
	// A role that is absent or null is not a string, and `!role` in the reference is true for
	// both — one of the corpus's cases supplies `null` against a `string | null | undefined`
	// parameter, which is corpus/README.md's first kind of null: the type admits it, so it is the
	// value, and it means "no role".
	role, _ := input.Field("role").Str()
	parsed, ok := ParseRole(role)
	if !ok {
		return canonical.Null(), nil
	}
	return canonical.Object().
		Set("chain", canonical.String(parsed.Chain)).
		Set("link", canonical.String(parsed.Link)).
		Set("axis", optionalString(parsed.Axis, parsed.HasAxis)).
		Set("index", optionalNumber(parsed.Index, parsed.HasIndex)).
		Set("key", canonical.String(RoleKey(parsed))), nil
}

// optionalString is a value the reference writes as `x ?? null`.
func optionalString(value string, present bool) canonical.Value {
	if !present {
		return canonical.Null()
	}
	return canonical.String(value)
}

// optionalNumber is the same for a number.
func optionalNumber(value float64, present bool) canonical.Value {
	if !present {
		return canonical.Null()
	}
	return canonical.Number(value)
}
