// ECMAScript semantics that Go's standard library spells differently.
//
// Every function here exists because the reference edition calls something whose Go equivalent is
// *nearly* the same, and a contract built on the near-equivalent agrees on every input anybody
// thought to test and diverges on one nobody did. They are gathered in one package so there is one
// place to be right, and `cmd/selfcheck` refuses the standard-library spellings in the contracts
// so that reaching for the wrong one is caught rather than reviewed for.
//
// Finding 159, from the session that found this in all three existing editions at once:
//
//	A rule written down in one module is not a rule. The whitespace set was documented in
//	semver.rs and reached 2 of Rust's 12 trimming files; the other editions did no better. What
//	makes it hold is the gate — a disallowed method, a token scan, a deleted helper.
package js

import (
	"math"
	"strconv"
	"strings"
)

// spaceSet is ECMAScript's WhiteSpace ∪ LineTerminator: what String.prototype.trim removes.
//
// Twenty-five code points, and the three that matter are the three that differ from every
// neighbouring definition:
//
//   - **U+FEFF ZERO WIDTH NO-BREAK SPACE goes.** Go's strings.TrimSpace keeps it — unicode.IsSpace
//     does not include it — as do Rust's and Python's trims. A byte-order mark on the front of a
//     descriptor read from a file is removed by the reference and kept by a naive edition.
//   - **U+0085 NEXT LINE stays.** Go, Rust and Python all strip it. This is the direction that
//     cannot be fixed by "strip a bit more": an edition trimming it removes a character the
//     reference keeps, and the value that reaches the wire is shorter than the reference's.
//   - **U+00A0 NO-BREAK SPACE goes**, which an ASCII-only strip keeps.
//
// Written as code points rather than as the characters themselves because as characters it is
// twenty-five mostly-invisible glyphs that no editor renders distinguishably and no diff can be
// read — the Python edition records the same decision for the same reason.
var spaceSet = [...]rune{
	0x0009, // CHARACTER TABULATION
	0x000A, // LINE FEED
	0x000B, // LINE TABULATION
	0x000C, // FORM FEED
	0x000D, // CARRIAGE RETURN
	0x0020, // SPACE
	0x00A0, // NO-BREAK SPACE
	0x1680, // OGHAM SPACE MARK
	0x2000, // EN QUAD
	0x2001, // EM QUAD
	0x2002, // EN SPACE
	0x2003, // EM SPACE
	0x2004, // THREE-PER-EM SPACE
	0x2005, // FOUR-PER-EM SPACE
	0x2006, // SIX-PER-EM SPACE
	0x2007, // FIGURE SPACE
	0x2008, // PUNCTUATION SPACE
	0x2009, // THIN SPACE
	0x200A, // HAIR SPACE
	0x2028, // LINE SEPARATOR
	0x2029, // PARAGRAPH SEPARATOR
	0x202F, // NARROW NO-BREAK SPACE
	0x205F, // MEDIUM MATHEMATICAL SPACE
	0x3000, // IDEOGRAPHIC SPACE
	0xFEFF, // ZERO WIDTH NO-BREAK SPACE
}

// SpaceClass is the same set again, rendered for the inside of a Go regexp character class.
//
// Built from spaceSet rather than written out a second time, because a hand-copied second spelling
// of a twenty-five-character invisible set is finding 159 waiting to happen. Used as
// `"[" + js.SpaceClass + "]*"` wherever the reference writes `\s` — which in JavaScript is this
// set, and in Go's regexp is [\t\n\f\r ]: five characters out of twenty-five.
var SpaceClass = spaceClass()

func spaceClass() string {
	const hex = "0123456789ABCDEF"
	out := make([]byte, 0, len(spaceSet)*9)
	for _, space := range spaceSet {
		out = append(out, '\\', 'x', '{')
		for shift := 12; shift >= 0; shift -= 4 {
			out = append(out, hex[(space>>shift)&0xf])
		}
		out = append(out, '}')
	}
	return string(out)
}

// IsSpace reports whether a rune is in ECMAScript's whitespace set.
//
// Not unicode.IsSpace, which is a different set in both directions: it includes U+0085 and excludes
// U+FEFF.
func IsSpace(character rune) bool {
	// Linear over twenty-five sorted runes, and the early exit makes it two comparisons for the
	// ASCII characters that are almost all of the input. A binary search here would be harder to
	// read for no measurable gain at this size.
	for _, space := range spaceSet {
		if character == space {
			return true
		}
		if character < space {
			return false
		}
	}
	return false
}

// Trim is ECMAScript's String.prototype.trim.
func Trim(text string) string {
	return strings.TrimFunc(text, IsSpace)
}

// TrimStart is ECMAScript's String.prototype.trimStart.
func TrimStart(text string) string {
	return strings.TrimLeftFunc(text, IsSpace)
}

// TrimEnd is ECMAScript's String.prototype.trimEnd.
func TrimEnd(text string) string {
	return strings.TrimRightFunc(text, IsSpace)
}

// Truthy is JavaScript's notion of truth for the values a parsed JSON document can hold.
//
// The reference writes `if (value)` over fields whose type admits a string, a number or undefined,
// and the falsy set is not "empty or absent": the empty string is false, and so is 0, and so is
// NaN. An edition testing only for absence accepts a declared zero where the reference rejects it.
func Truthy(text string) bool { return text != "" }

// TruthyNumber is the same test for a number: 0, -0 and NaN are false.
func TruthyNumber(value float64) bool { return value != 0 && !math.IsNaN(value) }

// Round is JavaScript's Math.round.
//
// Not math.Round, which rounds halves **away from zero**: Math.round(-0.5) is -0 and math.Round
// gives -1. JavaScript rounds halves toward +∞, which is floor(x + 0.5) — with one exception that
// is why this is not written as that expression. At x = 0.49999999999999994, the largest double
// below a half, x + 0.5 rounds up to exactly 1.0 in binary floating point, so floor gives 1 where
// Math.round gives 0. The reference computes budgets and deadlines through this, and a millisecond
// either way is a different verdict.
func Round(value float64) float64 {
	if math.IsNaN(value) || math.IsInf(value, 0) {
		return value
	}
	floor := math.Floor(value)
	if value-floor >= 0.5 {
		return floor + 1
	}
	return floor
}

// NumberToString is ECMAScript's Number::toString — what `String(x)` and `${x}` produce.
//
// **It is not canonical.FormatNumber, and the difference is the point of having both.** Rule 3 of
// the canonical form says "always positional, never exponential", and ECMAScript says the opposite
// outside a window: a magnitude at or above 1e21, or below 1e-6, comes out as `1e+23`. So a number
// that reaches the *wire* goes through canonical.FormatNumber, and a number the reference
// interpolates into a *string* goes through this — `roleKey` builds `#${index}`, and an edition
// using the canonical writer there would spell a large ordinal as 24 digits where the reference
// spells it `1e+23`.
//
// The algorithm is the specification's, over the shortest round-tripping digits: with the value
// written as s × 10^(n−k), where s has k digits, it is positional when k ≤ n ≤ 21 or −6 < n ≤ 21,
// and exponential otherwise.
func NumberToString(value float64) string {
	if math.IsNaN(value) {
		return "NaN"
	}
	if math.IsInf(value, 1) {
		return "Infinity"
	}
	if math.IsInf(value, -1) {
		return "-Infinity"
	}
	if value == 0 {
		return "0" // covers -0, which String() also writes as "0"
	}

	sign := ""
	if value < 0 {
		sign = "-"
		value = -value
	}
	// Go's shortest 'e' form is the same digits ECMAScript's step 5 asks for: the fewest that
	// round-trip. Split into the digit string s and the exponent.
	rendered := strconv.FormatFloat(value, 'e', -1, 64)
	marker := strings.IndexByte(rendered, 'e')
	digits := strings.Replace(rendered[:marker], ".", "", 1)
	exponent, err := strconv.Atoi(rendered[marker+1:])
	if err != nil {
		// Unreachable: FormatFloat with 'e' always writes an exponent.
		return sign + rendered
	}
	k := len(digits)
	n := exponent + 1

	switch {
	case k <= n && n <= 21:
		return sign + digits + strings.Repeat("0", n-k)
	case 0 < n && n <= 21:
		return sign + digits[:n] + "." + digits[n:]
	case -6 < n && n <= 0:
		return sign + "0." + strings.Repeat("0", -n) + digits
	}
	// Exponential. ECMAScript writes the exponent with an explicit sign and no padding, so `e+23`
	// and `e-7` rather than Go's `e+23` and `e-07`.
	exponentSign := "+"
	magnitude := n - 1
	if magnitude < 0 {
		exponentSign = "-"
		magnitude = -magnitude
	}
	mantissa := digits
	if k != 1 {
		mantissa = digits[:1] + "." + digits[1:]
	}
	return sign + mantissa + "e" + exponentSign + strconv.Itoa(magnitude)
}

// BareNumber reads a JSON number the way the reference's type says it must be read.
//
// corpus/README.md states the rule as a property of the *format*: a `null` in a case's input is
// whatever the reference's type says it is. Where the type is a bare `number`, no caller could have
// supplied a null, so the only thing it can be is a value JSON cannot spell — and it reads as NaN.
// Where the type admits null, the null is the value and this function is the wrong one to call.
//
// It is a named function rather than an inline `if` at each site because the two readings are
// opposite errors: reading a declared absence as NaN discards it, and reading a NaN as an absence
// answers a different question from the one the expectation was computed against. A reader of a
// call site should see which of the two was chosen.
func BareNumber(present bool, value float64, isNumber bool) float64 {
	if !present || !isNumber {
		return math.NaN()
	}
	return value
}
