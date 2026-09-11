package js

import (
	"math"
	"math/big"
	"strings"
)

// ToFixed is ECMAScript's Number.prototype.toFixed.
//
// Not strconv.FormatFloat with a precision, and the difference is a tie-break. ECMAScript's step 6
// says: *let n be an integer for which n / 10^f − x is as close to zero as possible; if there are
// two such n, pick the **larger** n* — so a value exactly halfway rounds up, toward +∞ on the
// magnitude. Go's fixed-precision formatter rounds half to even. The two disagree on 99.8125 at
// three places: ECMAScript gives 99.813 and Go gives 99.812.
//
// Reachable? Barely — an exact tie needs the double to be exactly k/10^(f+1) with a trailing 5,
// which means a dyadic rational, and almost no conformance figure is one. It is written correctly
// anyway, because "the inputs that would expose this are rare" is the property that makes a
// divergence survive for years (finding 159's shape), and because the exact form is no harder: the
// value is converted to a rational, which is lossless for a float64, and rounded there.
func ToFixed(value float64, digits int) string {
	if math.IsNaN(value) {
		return "NaN"
	}
	// The specification falls back to ToString outside this range, and so does this.
	if math.IsInf(value, 0) || math.Abs(value) >= 1e21 {
		return NumberToString(value)
	}

	sign := ""
	if value < 0 {
		sign, value = "-", -value
	}

	// Exact: SetFloat64 is lossless, so the scaling and the comparison below are over the value the
	// double actually holds rather than over a decimal approximation of it.
	scaled := new(big.Rat).SetFloat64(value)
	if scaled == nil {
		return NumberToString(value)
	}
	power := new(big.Int).Exp(big.NewInt(10), big.NewInt(int64(digits)), nil)
	scaled.Mul(scaled, new(big.Rat).SetInt(power))
	// floor(x + 1/2) is "nearest, ties to the larger", which is the specification's rule once the
	// sign has been taken off.
	scaled.Add(scaled, big.NewRat(1, 2))
	rounded := new(big.Int).Quo(scaled.Num(), scaled.Denom())
	if scaled.Sign() < 0 && new(big.Int).Mul(rounded, scaled.Denom()).Cmp(scaled.Num()) != 0 {
		rounded.Sub(rounded, big.NewInt(1))
	}

	text := rounded.String()
	if digits == 0 {
		return sign + text
	}
	if len(text) <= digits {
		text = strings.Repeat("0", digits-len(text)+1) + text
	}
	cut := len(text) - digits
	// ECMAScript writes `-0.00` for a small negative rounding to zero, because the sign is
	// prepended after the digits are chosen rather than dropped with the magnitude.
	return sign + text[:cut] + "." + text[cut:]
}
