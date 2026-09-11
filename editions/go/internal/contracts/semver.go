package contracts

import (
	"regexp"
	"strconv"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// Minimal semver parsing, comparison and range satisfaction.
//
// It is a contract rather than a utility because of §54.2: preconditions became predicates the OS
// evaluates before a skill may start, `semver_satisfies` is one of the operators, and a robot
// deciding whether its runtime satisfies `^1.2.0` differently from the Hub that sold it the skill
// is the §31.2 failure — *a format with two implementations of its own definition is not a format.*

// ParsedVersion is a version taken apart.
type ParsedVersion struct {
	Major         float64
	Minor         float64
	Patch         float64
	Prerelease    string
	HasPrerelease bool
	Raw           string
}

// semverPattern is the reference's `/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/`, with `\d`
// written out as [0-9] for the reason embodiment.go's rolePattern gives.
var semverPattern = regexp.MustCompile(`^([0-9]+)\.([0-9]+)\.([0-9]+)(?:-([0-9A-Za-z.-]+))?$`)

// numericPattern is `/^\d+$/` from comparePrerelease, same respelling.
var numericPattern = regexp.MustCompile(`^[0-9]+$`)

// ParseVersion parses a version, reporting whether it is one.
func ParseVersion(input string) (ParsedVersion, bool) {
	trimmed := js.Trim(input)
	match := semverPattern.FindStringSubmatch(trimmed)
	if match == nil {
		return ParsedVersion{}, false
	}
	return ParsedVersion{
		Major:         number(match[1]),
		Minor:         number(match[2]),
		Patch:         number(match[3]),
		Prerelease:    match[4],
		HasPrerelease: js.Truthy(match[4]),
		// The reference stores `input.trim()`, not the untrimmed input. Worth naming because the
		// reference edition shipped the opposite mistake elsewhere — finding 162, *testing a value
		// and emitting it are two operations* — where identity.ts trimmed a digest to decide it
		// was not blank and put the untrimmed one on the wire.
		Raw: trimmed,
	}, true
}

// IsValidVersion reports whether a string is a version this edition can read.
func IsValidVersion(input string) bool {
	_, ok := ParseVersion(input)
	return ok
}

// number is JavaScript's Number() over a string the pattern has already proven is digits.
//
// A value beyond 2^53 loses precision here exactly as Number() does, which is the agreement wanted:
// an edition that parsed into a 64-bit integer would order two twenty-digit majors that the
// reference calls equal.
func number(digits string) float64 {
	value, err := strconv.ParseFloat(digits, 64)
	if err != nil {
		// Unreachable behind [0-9]+, and handled rather than dropped: R-15 has no exception for a
		// branch that cannot be taken today.
		return 0
	}
	return value
}

// comparePrerelease compares prerelease identifiers per semver §11: numeric before alphanumeric,
// longer wins ties.
//
// The string comparison at the end is a byte comparison in Go and a UTF-16 code-unit comparison in
// the reference. They agree here and only here: the pattern that produced these strings admits
// [0-9A-Za-z.-] and nothing else, so every character is ASCII and the two orderings coincide. An
// edition that reused this function for arbitrary text would not have that.
func comparePrerelease(left string, hasLeft bool, right string, hasRight bool) int {
	if hasLeft == hasRight && left == right {
		return 0
	}
	// A version without a prerelease outranks one with it.
	if !hasLeft {
		return 1
	}
	if !hasRight {
		return -1
	}

	leftParts := strings.Split(left, ".")
	rightParts := strings.Split(right, ".")
	length := len(leftParts)
	if len(rightParts) > length {
		length = len(rightParts)
	}

	for at := 0; at < length; at++ {
		if at >= len(leftParts) {
			return -1
		}
		if at >= len(rightParts) {
			return 1
		}
		leftPart, rightPart := leftParts[at], rightParts[at]
		leftNumeric := numericPattern.MatchString(leftPart)
		rightNumeric := numericPattern.MatchString(rightPart)
		if leftNumeric && rightNumeric {
			difference := number(leftPart) - number(rightPart)
			if difference != 0 {
				if difference < 0 {
					return -1
				}
				return 1
			}
			continue
		}
		if leftNumeric {
			return -1
		}
		if rightNumeric {
			return 1
		}
		if leftPart != rightPart {
			if leftPart < rightPart {
				return -1
			}
			return 1
		}
	}
	return 0
}

// CompareVersions returns -1 if left < right, 0 if equal, 1 if left > right.
//
// Two unparseable versions compare equal rather than erroring, which is the reference's behaviour
// and is load-bearing: `satisfies` has already refused an unparseable target before it gets here,
// and the callers that have not are asking a question to which "these are not ordered" is the
// honest answer.
func CompareVersions(left, right ParsedVersion) int {
	if left.Major != right.Major {
		if left.Major < right.Major {
			return -1
		}
		return 1
	}
	if left.Minor != right.Minor {
		if left.Minor < right.Minor {
			return -1
		}
		return 1
	}
	if left.Patch != right.Patch {
		if left.Patch < right.Patch {
			return -1
		}
		return 1
	}
	return comparePrerelease(left.Prerelease, left.HasPrerelease, right.Prerelease, right.HasPrerelease)
}

// CompareVersionStrings is the reference's `compareVersions(a: string, b: string)`: a pair that
// does not parse compares equal.
func CompareVersionStrings(left, right string) int {
	parsedLeft, okLeft := ParseVersion(left)
	parsedRight, okRight := ParseVersion(right)
	if !okLeft || !okRight {
		return 0
	}
	return CompareVersions(parsedLeft, parsedRight)
}

// comparator is one term of a range: an operator and the version it is about.
type comparator struct {
	op      string
	version ParsedVersion
}

// operators are the prefixes a comparator can carry, **longest first**.
//
// The order is the alternation's in the reference's `(>=|<=|>|<|\^|~|=)?`, and it is the whole
// correctness of this list: a regex alternation tries its branches left to right, so `>=` has to be
// offered before `>` or `>=1.2.0` parses as `>` applied to `=1.2.0`, which then fails to be a
// version and takes the comparator with it.
var operators = []string{">=", "<=", ">", "<", "^", "~", "="}

// lineTerminators is what JavaScript's `.` does not match.
//
// Go's `.` excludes only \n, so a pattern ending `(.+)$` accepts a carriage return where the
// reference rejects it. parseComparator is written out rather than as a regex partly for this: the
// difference is one character wide and there is no way to see it in a pattern string.
const lineTerminators = "\n\r  "

// parseComparator reads one token of a range.
//
// The reference writes `/^(>=|<=|>|<|\^|~|=)?\s*(.+)$/`, and this is that regex spelt out, because
// two of its three pieces mean something different in Go: `\s` is five characters rather than
// ECMAScript's twenty-five, and `.` excludes one line terminator rather than four. Written as code,
// each of those is a named check a reader can see; written as a pattern, both are invisible.
func parseComparator(token string) (comparator, bool) {
	rest := js.Trim(token)
	op := "="
	for _, candidate := range operators {
		if strings.HasPrefix(rest, candidate) {
			op = candidate
			rest = rest[len(candidate):]
			break
		}
	}
	rest = js.TrimStart(rest)
	// `(.+)` needs at least one character, and `.` matches none of the line terminators — so a
	// remainder holding one can never reach `$` and the whole match fails.
	if rest == "" || strings.ContainsAny(rest, lineTerminators) {
		return comparator{}, false
	}
	version, ok := ParseVersion(rest)
	if !ok {
		return comparator{}, false
	}
	return comparator{op: op, version: version}, true
}

func satisfiesComparator(target ParsedVersion, term comparator) bool {
	order := CompareVersions(target, term.version)
	switch term.op {
	case ">=":
		return order >= 0
	case ">":
		return order > 0
	case "<=":
		return order <= 0
	case "<":
		return order < 0
	case "=":
		return order == 0
	case "^":
		// Caret: compatible within the leftmost non-zero component.
		if order < 0 {
			return false
		}
		base := term.version
		if base.Major > 0 {
			return target.Major == base.Major
		}
		if base.Minor > 0 {
			return target.Major == 0 && target.Minor == base.Minor
		}
		return target.Major == 0 && target.Minor == 0 && target.Patch == base.Patch
	case "~":
		// Tilde: patch-level changes within the same minor.
		if order < 0 {
			return false
		}
		return target.Major == term.version.Major && target.Minor == term.version.Minor
	default:
		return false
	}
}

// tokenPattern is the reference's tokeniser:
//
//	/(?:>=|<=|>|<|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?/g
//
// It tokenises on the operator rather than on whitespace, so ">= 1.2.0" and ">=1.2.0" both parse.
// `\s` is written as ECMAScript's set and `\d` as [0-9]; those two respellings are the difference
// between reading `>=` followed by a non-breaking space as a comparator and not seeing it at all.
var tokenPattern = regexp.MustCompile(
	`(?:>=|<=|>|<|\^|~|=)?[` + js.SpaceClass + `]*[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?`)

// rangeGroup is one `||` alternative. Unreadable ones are kept as a group that did not parse rather
// than dropped, because `satisfies` and `isValidRange` want different answers about them.
type rangeGroup struct {
	comparators []comparator
	ok          bool
}

// parseRange splits a range into its `||` groups.
//
// `wildcard` is the reference's `null` for the whole result: the range is `*`, `any` or empty,
// every version satisfies it, and there is nothing to compare against.
func parseRange(text string) (groups []rangeGroup, wildcard bool) {
	trimmed := js.Trim(text)
	if !js.Truthy(trimmed) || trimmed == "*" || js.ToLower(trimmed) == "any" {
		return nil, true
	}
	for _, part := range strings.Split(trimmed, "||") {
		tokens := tokenPattern.FindAllString(js.Trim(part), -1)
		if len(tokens) == 0 {
			groups = append(groups, rangeGroup{})
			continue
		}
		group := rangeGroup{ok: true}
		for _, token := range tokens {
			term, ok := parseComparator(token)
			if !ok {
				group = rangeGroup{}
				break
			}
			group.comparators = append(group.comparators, term)
		}
		groups = append(groups, group)
	}
	return groups, false
}

// Satisfies reports whether a version satisfies a range.
//
// Supports `*`, space-separated AND comparators (`>=1.2.0 <2.0.0`) and `||` OR groups.
func Satisfies(version, rangeText string) bool {
	target, ok := ParseVersion(version)
	if !ok {
		return false
	}
	groups, wildcard := parseRange(rangeText)
	if wildcard {
		return true
	}
	for _, group := range groups {
		if !group.ok {
			continue
		}
		all := true
		for _, term := range group.comparators {
			if !satisfiesComparator(target, term) {
				all = false
				break
			}
		}
		if all {
			return true
		}
	}
	return false
}

// IsValidRange reports whether a range is one this edition can read at all.
//
// `Satisfies` answers false both for "this version is outside that window" and for "that is not a
// window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is a
// skill nobody can ever run here, and its publisher is the one who has to hear about it. A battery
// that is too low is a different sentence with a different remedy.
func IsValidRange(rangeText string) bool {
	groups, wildcard := parseRange(rangeText)
	if wildcard {
		return true
	}
	for _, group := range groups {
		if !group.ok {
			return false
		}
	}
	return true
}

// ── the corpus ──────────────────────────────────────────────────────────────

func semverCompare(input canonical.Value) (canonical.Value, error) {
	left, _ := input.Field("a").Str()
	right, _ := input.Field("b").Str()
	return canonical.Object().
		Set("compare", canonical.Number(float64(CompareVersionStrings(left, right)))).
		Set("a", shapeVersion(left)).
		Set("b", shapeVersion(right)), nil
}

func shapeVersion(text string) canonical.Value {
	parsed, ok := ParseVersion(text)
	if !ok {
		return canonical.Null()
	}
	return canonical.Object().
		Set("major", canonical.Number(parsed.Major)).
		Set("minor", canonical.Number(parsed.Minor)).
		Set("patch", canonical.Number(parsed.Patch)).
		Set("prerelease", optionalString(parsed.Prerelease, parsed.HasPrerelease)).
		Set("raw", canonical.String(parsed.Raw))
}

func semverSatisfies(input canonical.Value) (canonical.Value, error) {
	version, _ := input.Field("version").Str()
	rangeText, _ := input.Field("range").Str()
	return canonical.Object().
		Set("version", canonical.String(version)).
		Set("range", canonical.String(rangeText)).
		Set("satisfies", canonical.Bool(Satisfies(version, rangeText))).
		Set("valid_version", canonical.Bool(IsValidVersion(version))).
		Set("valid_range", canonical.Bool(IsValidRange(rangeText))), nil
}
