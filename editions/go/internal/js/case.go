package js

import "strings"

// ToUpper is ECMAScript's String.prototype.toUpperCase.
//
// Not strings.ToUpper, which is the *simple* mapping: it leaves `ß` and the `ﬁ` ligature alone
// where the reference turns them into `SS` and `FI`. Both reach the corpus through the pairing
// code in identity.ts, which upper-cases before it counts characters — so an edition using the
// simple mapping reads `ß234567` as seven characters where the reference reads eight, and refuses
// a code the reference accepts.
//
// The tables are in unicode_case.go, read out of the engine the reference runs on by
// tools/unicode-case.ts. Upper-casing has no context rule.
func ToUpper(text string) string {
	return mapRunes(text, func(_ string, _ int, ch rune) (string, bool) {
		return lookup(upperFrom, upperTo, ch)
	})
}

// ToLower is ECMAScript's String.prototype.toLowerCase.
//
// Not strings.ToLower, for two reasons rather than one. It is the simple mapping — U+0130 CAPITAL
// I WITH DOT ABOVE lower-cases to two code points here and to one there — and it has no
// final-sigma rule, so "ΟΔΟΣ" comes back with a medial σ where the reference gives ς.
//
// U+212A KELVIN SIGN is the character that makes this matter inside grammars everyone assumes are
// ASCII: it lower-cases to a plain `k`, so `nec` + U+212A + `.link.pitch` parses as the role
// `neck`. Finding 161 is that "every identifier is ASCII, so ASCII lowercasing is exact" was false
// twice — the reference maps *before* it checks.
func ToLower(text string) string {
	return mapRunes(text, func(whole string, at int, ch rune) (string, bool) {
		if ch == capitalSigma {
			return sigma(whole, at), true
		}
		return lookup(lowerFrom, lowerTo, ch)
	})
}

// capitalSigma is Σ, the one code point whose lower-case mapping depends on what surrounds it.
const capitalSigma = 'Σ'

// finalSigma is ς, and medialSigma is σ.
const (
	finalSigma  = "ς"
	medialSigma = "σ"
)

// mapRunes walks the string once, applying convert to each rune.
//
// The fast path matters and is not premature: the overwhelming majority of strings that reach here
// are ASCII identifiers with no mapping at all, and building a new string for each of them would
// allocate once per role, per channel and per version in every contract. The scan returns the
// input unchanged when nothing mapped.
func mapRunes(text string, convert func(whole string, at int, ch rune) (string, bool)) string {
	var out strings.Builder
	// Tracked in a flag rather than by asking whether the builder is empty: a mapping to the empty
	// string at index 0 would leave the builder empty and silently drop the rest of the string.
	// No Unicode mapping is empty today, which is exactly why that bug would survive review.
	building := false
	for at, character := range text {
		mapped, changed := convert(text, at, character)
		if !changed {
			if building {
				out.WriteRune(character)
			}
			continue
		}
		if !building {
			building = true
			out.Grow(len(text) + 8)
			out.WriteString(text[:at])
		}
		out.WriteString(mapped)
	}
	if !building {
		return text
	}
	return out.String()
}

// sigma applies the Final_Sigma condition.
//
// A capital sigma lower-cases to ς when a cased character precedes it and none follows, skipping
// case-ignorable characters in both directions. The subtlety, and the reason the vectors in
// case_vectors_test.go exist rather than a reading of the specification: a character can be
// **both** Cased and Case_Ignorable — U+02B0 MODIFIER LETTER SMALL H is — and the rule read
// literally would count it as the preceding letter. The engine steps over it. So Case_Ignorable is
// tested first in both directions, and the vectors are what proved which way round that goes
// (finding 163).
func sigma(text string, at int) string {
	before := scan(text[:at], false)
	after := scan(text[at+len(string(capitalSigma)):], true)
	if before && !after {
		return finalSigma
	}
	return medialSigma
}

// scan reports whether the nearest character that is not case-ignorable is cased. `forward` picks
// the direction; the text given is already the part on that side of the sigma.
func scan(text string, forward bool) bool {
	runes := []rune(text)
	if forward {
		for _, ch := range runes {
			if inRanges(caseIgnorableRanges, ch) {
				continue
			}
			return inRanges(casedRanges, ch)
		}
		return false
	}
	for at := len(runes) - 1; at >= 0; at-- {
		if inRanges(caseIgnorableRanges, runes[at]) {
			continue
		}
		return inRanges(casedRanges, runes[at])
	}
	return false
}

// runeRange is one inclusive span of a Unicode property, as the generated tables hold them.
type runeRange struct {
	Lo rune
	Hi rune
}

// inRanges binary-searches the ascending, non-overlapping ranges the generator emits.
func inRanges(ranges []runeRange, value rune) bool {
	low, high := 0, len(ranges)
	for low < high {
		middle := (low + high) / 2
		switch {
		case value < ranges[middle].Lo:
			high = middle
		case value > ranges[middle].Hi:
			low = middle + 1
		default:
			return true
		}
	}
	return false
}

// lookup binary-searches a mapping table, returning the replacement and whether there was one.
func lookup(from []rune, to []string, value rune) (string, bool) {
	low, high := 0, len(from)
	for low < high {
		middle := (low + high) / 2
		switch {
		case value < from[middle]:
			high = middle
		case value > from[middle]:
			low = middle + 1
		default:
			return to[middle], true
		}
	}
	return "", false
}
