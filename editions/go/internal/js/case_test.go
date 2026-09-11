package js

import (
	"sort"
	"testing"
)

// TestCaseVectors converts 219 whole strings the engine converted and compares.
//
// The tables map single code points and cannot be wrong about a string on their own. The code that
// walks a string with them, and applies the final-sigma rule, can — and these are what would say
// so. Finding 163: *a generated table needs vectors from the same generator*, recorded when the
// literal reading of the sigma rule turned out to disagree with the engine about a character that
// is both Cased and Case_Ignorable.
func TestCaseVectors(t *testing.T) {
	if unicodeVersion != vectorsUnicodeVersion {
		t.Fatalf("the tables are Unicode %s and the vectors are Unicode %s: regenerate both with "+
			"`node tools/unicode-case.ts` under one Node", unicodeVersion, vectorsUnicodeVersion)
	}
	for _, vector := range caseVectors {
		actual := ToLower(vector.In)
		name := "toLowerCase"
		if vector.Upper {
			actual = ToUpper(vector.In)
			name = "toUpperCase"
		}
		if actual != vector.Out {
			t.Errorf("%s(%s)\n  expected %s\n  actual   %s", name, escape(vector.In), escape(vector.Out), escape(actual))
		}
	}
}

// TestCaseMappingsThatReachTheCorpus pins the three characters the 2026-09-11 session names as
// having landed inside grammars everyone had assumed were ASCII (finding 161).
func TestCaseMappingsThatReachTheCorpus(t *testing.T) {
	// U+212A KELVIN SIGN lower-cases to a plain ASCII `k`, so this is the role `neck`.
	if got := ToLower("necK.link.pitch"); got != "neck.link.pitch" {
		t.Errorf("U+212A must lower-case to k, got %q", got)
	}
	// Two upper-case mappings that produce more characters than they consume. An edition using a
	// simple mapping leaves both alone and then miscounts the length of a pairing code.
	if got := ToUpper("ß234567"); got != "SS234567" {
		t.Errorf("sharp s must upper-case to SS, got %q", got)
	}
	if got := ToUpper("ﬁx"); got != "FIX" {
		t.Errorf("the fi ligature must upper-case to FI, got %q", got)
	}
	// The final-sigma rule, in the two positions that tell it apart from the simple mapping.
	if got := ToLower("ΟΔΟΣ"); got != "οδος" {
		t.Errorf("a trailing sigma must be final, got %q", got)
	}
	if got := ToLower("ΣΟΣ"); got != "σος" {
		t.Errorf("a leading sigma must be medial and a trailing one final, got %q", got)
	}
}

// TestTheThreeCharactersThatDifferFromEveryNeighbouringDefinition pins the whitespace set at the
// three code points the 2026-09-11 session found wrong in all three existing editions at once.
//
// Written as an assertion rather than as a comment because the comment already existed, in
// semver.rs, and reached two of Rust's twelve trimming files (finding 159). Every character here is
// written as an escape: as glyphs they are invisible, and a test whose subject cannot be seen in
// the diff is one nobody can review.
func TestTheThreeCharactersThatDifferFromEveryNeighbouringDefinition(t *testing.T) {
	// U+FEFF is removed. Go's strings.TrimSpace keeps it, as do Rust's and Python's trims.
	if got := Trim("\uFEFFneck.pitch\uFEFF"); got != "neck.pitch" {
		t.Errorf("U+FEFF must be trimmed, got %q", got)
	}
	// U+0085 is kept. Go, Rust and Python all strip it — the one direction that cannot be fixed by
	// stripping a little more.
	if got := Trim("\u0085neck.pitch\u0085"); got != "\u0085neck.pitch\u0085" {
		t.Errorf("U+0085 must be kept, got %q", got)
	}
	// U+00A0 is removed, which an ASCII-only strip keeps.
	if got := Trim("\u00A0neck.pitch\u00A0"); got != "neck.pitch" {
		t.Errorf("U+00A0 must be trimmed, got %q", got)
	}
	// Finding 160's probe design: a stripper that removes only the outer character of a must-remove
	// pair still has to fail, so the pair is tested as a pair.
	if got := Trim("\uFEFF\u00A0neck.pitch\u00A0\uFEFF"); got != "neck.pitch" {
		t.Errorf("U+FEFF and U+00A0 together must both be trimmed, got %q", got)
	}
	// And the kept probe is alone, for the same finding's other half: a pair of must-keep
	// characters lets an edition that strips only the outer one agree by accident.
	if got := Trim(" \u0085 "); got != "\u0085" {
		t.Errorf("a lone U+0085 between spaces must survive, got %q", got)
	}
}

// TestCaseTablesSorted checks the order the binary searches assume.
//
// A table that lost its order would still answer most lookups — binary search over nearly-sorted
// data is right far more often than it is wrong — which is the kind of defect that shows up as one
// failing corpus case in a year's time.
func TestCaseTablesSorted(t *testing.T) {
	for _, table := range []struct {
		name string
		from []rune
		to   []string
	}{
		{"upper", upperFrom, upperTo},
		{"lower", lowerFrom, lowerTo},
	} {
		if len(table.from) != len(table.to) {
			t.Errorf("%s: %d code points and %d mappings", table.name, len(table.from), len(table.to))
		}
		if !sort.SliceIsSorted(table.from, func(left, right int) bool { return table.from[left] < table.from[right] }) {
			t.Errorf("%s: the code points are not ascending", table.name)
		}
	}
	for _, table := range []struct {
		name   string
		ranges []runeRange
	}{
		{"cased", casedRanges},
		{"caseIgnorable", caseIgnorableRanges},
	} {
		for at, span := range table.ranges {
			if span.Lo > span.Hi {
				t.Errorf("%s[%d] is empty: U+%04X..U+%04X", table.name, at, span.Lo, span.Hi)
			}
			// Adjacent rather than merely ascending: the generator coalesces touching spans, so a
			// pair that could have been one is a sign it did not run.
			if at > 0 && table.ranges[at-1].Hi+1 >= span.Lo {
				t.Errorf("%s[%d] is not after %d: U+%04X..U+%04X then U+%04X..U+%04X",
					table.name, at, at-1, table.ranges[at-1].Lo, table.ranges[at-1].Hi, span.Lo, span.Hi)
			}
		}
	}
}

// TestRoundIsJavaScripts pins Math.round at the inputs that tell it apart from every other
// rounding function.
func TestRoundIsJavaScripts(t *testing.T) {
	// Halves go toward +inf, not away from zero: math.Round(-0.5) is -1.
	if got := Round(-0.5); got != 0 {
		t.Errorf("Round(-0.5) = %v, want 0", got)
	}
	if got := Round(0.5); got != 1 {
		t.Errorf("Round(0.5) = %v, want 1", got)
	}
	if got := Round(-1.5); got != -1 {
		t.Errorf("Round(-1.5) = %v, want -1", got)
	}
	// The largest double below a half. floor(x+0.5) gives 1 here because the addition rounds up;
	// Math.round gives 0.
	if got := Round(0.49999999999999994); got != 0 {
		t.Errorf("Round(0.49999999999999994) = %v, want 0", got)
	}
}

// escape renders a string as code points, because the vectors are mostly invisible characters and a
// test failure printing them raw says nothing at all.
func escape(text string) string {
	out := make([]byte, 0, len(text)*8)
	const hex = "0123456789ABCDEF"
	for _, character := range text {
		out = append(out, 'U', '+')
		for shift := 16; shift >= 0; shift -= 4 {
			digit := (character >> shift) & 0xf
			if shift > 12 && digit == 0 {
				continue
			}
			out = append(out, hex[digit])
		}
		out = append(out, ' ')
	}
	return string(out)
}
