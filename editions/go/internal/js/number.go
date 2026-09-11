package js

import (
	"math"
	"math/big"
	"regexp"
	"strconv"
)

// StringToNumber is ECMAScript's `Number(someString)`, returning NaN for a string that is not one.
//
// Not strconv.ParseFloat, and the two disagree in both directions on inputs the corpus supplies.
// `corpus/predicate/evaluate.json`'s context carries `"0x1f"`, `"1e3"` and `"Infinity"` in adjacent
// fields for exactly this reason — the numeric comparators run everything through this, so a robot
// reading `"0x1f"` as 31 and an edition reading it as NaN disagree about whether a precondition
// holds.
//
//	                ECMAScript      Go's ParseFloat
//	"0x1f"          31              an error (Go wants a `p` exponent on a hex float)
//	"Infinity"      +∞              +∞, and also accepts "inf", "INF", "infinity"
//	"NaN"           NaN             NaN, which ECMAScript's grammar does not admit at all
//	""              0               an error
//	"   "           0               an error
//	"1_000"         NaN             1000
//	"0b101"         5               an error
//
// Every difference above is a silent one: both sides produce *a* number for most inputs, and the
// ones where they differ are the ones nobody writes a test for.
func StringToNumber(text string) float64 {
	// StrWhiteSpace is the same set trim removes, which is why this calls Trim rather than
	// stripping ASCII spaces.
	trimmed := Trim(text)
	// The empty string is 0, not NaN. This is the branch that makes `Number("")` a famous
	// JavaScript question, and it is load-bearing here: `toNumber` in predicate.ts guards it with
	// a `trim() !== ''` test, so a caller that skipped the guard would turn a blank field into a
	// comparison against zero.
	if trimmed == "" {
		return 0
	}

	if infinityPattern.MatchString(trimmed) {
		if trimmed[0] == '-' {
			return math.Inf(-1)
		}
		return math.Inf(1)
	}

	// The non-decimal literals take no sign in this grammar: `Number("-0x10")` is NaN, where a
	// hand-written parser that stripped the sign first would answer -16.
	for _, form := range nonDecimal {
		if form.pattern.MatchString(trimmed) {
			return baseToFloat(trimmed[2:], form.base)
		}
	}

	if !decimalPattern.MatchString(trimmed) {
		return math.NaN()
	}
	value, err := strconv.ParseFloat(trimmed, 64)
	if err != nil {
		// Out of range is the one error the pattern still admits, and ParseFloat returns ±Inf with
		// it — which is what ECMAScript gives for `Number("1e400")` too. Any other error would be a
		// string the pattern should not have accepted, and NaN is the honest answer to that.
		if numeric, ok := err.(*strconv.NumError); ok && numeric.Err == strconv.ErrRange {
			return value
		}
		return math.NaN()
	}
	return value
}

var (
	infinityPattern = regexp.MustCompile(`^[+-]?Infinity$`)
	// ECMAScript's StrDecimalLiteral: a sign, digits with an optional fraction or a bare fraction,
	// and an optional exponent. Written with [0-9] rather than the shorthand for the reason
	// internal/contracts refuses the shorthand.
	decimalPattern = regexp.MustCompile(`^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$`)

	nonDecimal = []struct {
		pattern *regexp.Regexp
		base    int
	}{
		{regexp.MustCompile(`^0[xX][0-9a-fA-F]+$`), 16},
		{regexp.MustCompile(`^0[oO][0-7]+$`), 8},
		{regexp.MustCompile(`^0[bB][01]+$`), 2},
	}
)

// baseToFloat converts an arbitrarily long non-decimal literal, rounding the way ECMAScript does.
//
// Through big.Int rather than strconv.ParseUint because the grammar puts no length limit on the
// digits: `Number("0x" + "f".repeat(20))` is a finite double in ECMAScript and an overflow error in
// any fixed-width parse. big.Float's Float64 rounds to nearest-even, which is what the conversion
// from an exact mathematical value to a double is defined to do.
func baseToFloat(digits string, base int) float64 {
	whole, ok := new(big.Int).SetString(digits, base)
	if !ok {
		// Unreachable behind the patterns above, and answered rather than dropped.
		return math.NaN()
	}
	value, _ := new(big.Float).SetInt(whole).Float64()
	return value
}
