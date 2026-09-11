// The traps the corpus structurally cannot reach, and the checks on the runner itself.
//
//	go run ./cmd/selfcheck
//
// Three kinds of check, and each is here because the corpus cannot hold it:
//
//  1. **A source scan** refusing the standard-library spellings that are *nearly* right. §6.1 ranks
//     gates, and a test that scans the source is the fourth-strongest — the one for conventions no
//     type system expresses. Finding 159 is why it exists at all: *a rule written down in one module
//     is not a rule*, and the whitespace rule reached 2 of Rust's 12 trimming files while it was
//     only a comment.
//
//  2. **The runner's own accounting**, over synthetic corpora built here. The four buckets a report
//     prints are the sentence §31.4 asks this edition for, and no corpus case exercises them: every
//     real run has the same 1,199 cases and cannot show what happens when a handler is missing, or
//     declines, or panics.
//
//  3. **The canonical writer at its edges.** The round trip in cmd/roundtrip proves the writer
//     against every number the corpus holds, which is a strong statement about the numbers that
//     occur and none at all about the ones that do not.
package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/corpus"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

func main() {
	failures := 0
	failures += sourceScan()
	failures += runnerAccounting()
	failures += numberEdges()

	if failures > 0 {
		fmt.Fprintf(os.Stderr, "\nself-check: %d failed\n", failures)
		os.Exit(1)
	}
	fmt.Println("self-check: all checks passed")
}

func fail(format string, args ...any) int {
	fmt.Fprintf(os.Stderr, "  %s\n", fmt.Sprintf(format, args...))
	return 1
}

// ── 1. the source scan ──────────────────────────────────────────────────────

// banned is the standard-library spelling, and what to reach for instead.
//
// Every one of these is *nearly* the reference's behaviour, which is exactly what makes it
// dangerous: an edition using it agrees on every input anybody thought to test. The right spelling
// is in internal/js, which is the one package exempt from this scan because it is where the right
// spellings are written.
var banned = map[string]string{
	"strings.TrimSpace":   "js.Trim — Go's set keeps U+FEFF and strips U+0085; ECMAScript's does the opposite",
	"strings.TrimLeft":    "js.TrimStart, for the same reason (and TrimLeft takes a cutset, not a set)",
	"strings.TrimRight":   "js.TrimEnd, for the same reason",
	"strings.Fields":      "a split on js.IsSpace; Fields uses unicode.IsSpace",
	"strings.ToLower":     "js.ToLower — Go's is the simple mapping and has no final-sigma rule",
	"strings.ToUpper":     "js.ToUpper — Go's simple mapping leaves the sharp s and the ligatures alone",
	"strings.EqualFold":   "an explicit js.ToLower on both sides; EqualFold is a third case relation",
	"strings.Title":       "js.ToUpper on the first rune; Title is deprecated and Unicode-naive",
	"unicode.IsSpace":     "js.IsSpace — a different set in both directions",
	"unicode.ToLower":     "js.ToLower",
	"unicode.ToUpper":     "js.ToUpper",
	"math.Round":          "js.Round — math.Round takes halves away from zero, Math.round toward +inf",
	"json.Marshal":        "canonical.Encode — encoding/json is not the canonical form",
	"json.Unmarshal":      "canonical.Decode — encoding/json loses insertion order and absent-vs-null",
	"json.NewEncoder":     "canonical.Encode",
	"json.NewDecoder":     "canonical.Decode",
	"strconv.FormatFloat": "canonical.FormatNumber for the wire, or js.NumberToString for a string the reference interpolates — rule 3 is neither of Go's float formats",
}

// bannedInPattern is what may not appear inside a regexp literal.
//
// `\d`, `\s` and `\w` mean different sets in different dialects, and the corpus now holds a case
// for each: a range in Arabic-Indic digits, a comparator behind a non-breaking space. Go happens to
// agree with ECMAScript about `\d` and disagrees about `\s` — and "happens to agree" is not
// something a reader can see, so both are spelt out.
var bannedInPattern = map[string]string{
	`\d`: "[0-9] — `\\d` is ASCII in Go and in ECMAScript and is neither in Python; spell it out",
	`\s`: "[ + js.SpaceClass + ] — `\\s` is 5 characters in Go and 25 in ECMAScript",
	`\w`: "an explicit class — `\\w` differs by dialect the same way",
}

// exempt is where the right spellings are defined, and so the one place the wrong ones appear.
//
// §6.3: a carve-out names files, never wildcards, and its length is reported so it cannot quietly
// triple.
var exempt = map[string]string{
	"internal/js/js.go":           "defines Trim, IsSpace, Round and SpaceClass",
	"internal/js/case.go":         "defines ToUpper and ToLower",
	"internal/js/case_test.go":    "tests them, including against the spellings they replace",
	"internal/canonical/write.go": "defines FormatNumber, which is the one place rule 3 lives",
	"internal/canonical/parse.go": "defines Decode",
	"internal/canonical/value.go": "defines the value model the others are written against",
	"cmd/selfcheck/main.go":       "is this scan, and names every spelling it refuses",
}

func sourceScan() int {
	failures := 0
	checked := 0
	root := "."
	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if info.IsDir() || !strings.HasSuffix(path, ".go") {
			return nil
		}
		relative := filepath.ToSlash(path)
		relative = strings.TrimPrefix(relative, "./")
		if _, skip := exempt[relative]; skip {
			return nil
		}
		checked++
		failures += scanFile(relative)
		return nil
	})
	if err != nil {
		return fail("walking the source tree: %v", err)
	}
	if checked == 0 {
		return fail("the source scan found no files, which means it looked in the wrong place")
	}
	fmt.Printf("source scan  %d files, %d exemptions, %d refusals\n", checked, len(exempt), failures)
	return failures
}

func scanFile(path string) int {
	fileSet := token.NewFileSet()
	parsed, err := parser.ParseFile(fileSet, path, nil, 0)
	if err != nil {
		return fail("%s: %v", path, err)
	}
	failures := 0
	ast.Inspect(parsed, func(node ast.Node) bool {
		switch typed := node.(type) {
		case *ast.SelectorExpr:
			// A call written as a selector is what the ban is about. Matched on the AST rather than
			// on the text so that a comment naming `strings.ToLower` — as several in this edition
			// deliberately do, to say why it is not used — is not a violation.
			if identifier, ok := typed.X.(*ast.Ident); ok {
				spelling := identifier.Name + "." + typed.Sel.Name
				if instead, refused := banned[spelling]; refused {
					position := fileSet.Position(typed.Pos())
					failures += fail("%s:%d: %s — use %s", path, position.Line, spelling, instead)
				}
			}
		case *ast.BasicLit:
			if typed.Kind != token.STRING {
				return true
			}
			// Only literals that are regexp patterns, which in this edition are the ones passed to
			// regexp.MustCompile — checked by looking at the text for the escape, since a pattern
			// assembled by concatenation is still a pattern.
			for escape, instead := range bannedInPattern {
				if strings.Contains(typed.Value, escape) && strings.Contains(typed.Value, "^") {
					position := fileSet.Position(typed.Pos())
					failures += fail("%s:%d: %s in a pattern — use %s", path, position.Line, escape, instead)
				}
			}
		}
		return true
	})
	return failures
}

// ── 2. the runner's accounting ──────────────────────────────────────────────

// runnerAccounting drives the runner over corpora built here, so the four buckets it prints are
// checked rather than trusted.
//
// The floor this protects is finding-shaped: an edition whose handler was deleted answers nothing,
// fails nothing, and exits 0, because the cases move from *passed* into *not implemented*. Every
// case below asserts one of the four numbers the runner would have to get wrong for that to go
// unnoticed.
func runnerAccounting() int {
	failures := 0

	files := []corpus.File{
		{Corpus: "alpha", Name: "one", Cases: []corpus.Case{
			{ID: "passes", Note: "n", Expected: `{"a":1}`},
			{ID: "fails", Note: "n", Expected: `{"a":2}`},
		}},
		{Corpus: "alpha", Name: "two", Cases: []corpus.Case{{ID: "declined", Note: "n", Expected: "null"}}},
		{Corpus: "beta", Name: "one", Cases: []corpus.Case{{ID: "unhandled", Note: "n", Expected: "null"}}},
		{Corpus: "gamma", Name: "one", Cases: []corpus.Case{{ID: "panics", Note: "n", Expected: "null"}}},
	}
	dispatch := corpus.Dispatch{
		"alpha/one": func(canonical.Value) (canonical.Value, error) {
			return canonical.Object().Set("a", canonical.Number(1)), nil
		},
		"gamma/one": func(canonical.Value) (canonical.Value, error) {
			var empty []int
			_ = empty[3] // a handler with a bug in it, which must not take the runner down
			return canonical.Null(), nil
		},
	}

	report := corpus.Run(files, dispatch)
	expect := func(name string, got, want int) {
		if got != want {
			failures += fail("runner accounting: %s is %d, want %d", name, got, want)
		}
	}
	expect("passed", report.Passed, 1)
	// One wrong answer, and one panic counted as a failure rather than as an absence.
	expect("failed", report.Failed, 2)
	expect("unanswered", report.Unanswered, 2)
	expect("total", report.Total(), 5)

	if len(report.Complete) != 0 {
		failures += fail("runner accounting: complete is %v, want none — alpha has a red case and beta none", report.Complete)
	}
	if want := []string{"beta"}; !sameStrings(report.MissingHandlers, want) {
		failures += fail("runner accounting: missing handlers are %v, want %v", report.MissingHandlers, want)
	}
	if want := []string{"alpha/two"}; !sameStrings(report.DeclinedFiles, want) {
		failures += fail("runner accounting: declined files are %v, want %v — a corpus with no handler "+
			"at all is named once, not once per file", report.DeclinedFiles, want)
	}
	if want := []string{"alpha", "gamma"}; !sameStrings(report.Partial, want) {
		failures += fail("runner accounting: partial is %v, want %v", report.Partial, want)
	}

	// And the panic is reported as the case it happened in, not as a line in a stack trace nobody
	// can map back to a vector. Finding 164: a runner that crashes describing a failure hides it.
	found := false
	for _, failure := range report.Failures {
		if failure.File == "gamma/one" && failure.Case == "panics" && failure.Err != nil {
			found = true
		}
	}
	if !found {
		failures += fail("runner accounting: the panicking handler produced no named failure")
	}

	fmt.Printf("runner       4 buckets over %d synthetic cases\n", report.Total())
	return failures
}

func sameStrings(got, want []string) bool {
	if len(got) != len(want) {
		return false
	}
	for at := range got {
		if got[at] != want[at] {
			return false
		}
	}
	return true
}

// ── 3. the canonical writer's edges ─────────────────────────────────────────

// numberEdges pins rule 3 at the values the corpus does not happen to contain.
//
// The round trip proves the writer against every number that *occurs*. These are the ones that
// would occur on the first day somebody put a picojoule or a nanosecond in a manifest, and each is
// a place where a language's default float printer does something rule 3 forbids.
func numberEdges() int {
	failures := 0
	for _, vector := range []struct {
		value float64
		want  string
	}{
		{0, "0"},
		{-0, "0"}, // rule 3 writes negative zero as `0`
		{1, "1"},
		{-1, "-1"},
		{1e21, "1000000000000000000000"}, // Go and ECMAScript both reach for an exponent here
		{1e-7, "0.0000001"},              // and here, which is why formatNumber exists at all
		{-4.4e-1, "-0.44"},
		{0.1, "0.1"},
		{9007199254740991, "9007199254740991"}, // 2^53 - 1, the last exact integer
		{9007199254740994, "9007199254740994"}, // above 2^53: no longer the integer branch
		// The largest double. Its shortest round-tripping form is 1.7976931348623157e+308, so
		// positional is those 17 digits followed by 308-16 zeros. Written as the rule rather than
		// as a 309-character literal, because a literal that long is one nobody can check.
		{1.7976931348623157e308, "17976931348623157" + strings.Repeat("0", 292)},
		// And the smallest subnormal: 5e-324 is one significant digit at n = -323.
		{5e-324, "0." + strings.Repeat("0", 323) + "5"},
		{0.30000000000000004, "0.30000000000000004"}, // the shortest decimal that round-trips
	} {
		got, err := canonical.FormatNumber(vector.value)
		if err != nil {
			failures += fail("FormatNumber(%v): %v", vector.value, err)
			continue
		}
		if got != vector.want {
			failures += fail("FormatNumber(%v) = %s, want %s", vector.value, got, vector.want)
		}
	}

	// Non-finite values are refused rather than written as null, which is the distinction between
	// "this was not measured" and "this was measured as nothing".
	for _, value := range []float64{
		positiveInfinity(), negativeInfinity(), notANumber(),
	} {
		if _, err := canonical.FormatNumber(value); err == nil {
			failures += fail("FormatNumber(%v) produced a value; a non-finite number must be refused", value)
		}
	}

	// The whitespace set, from the other direction: the class handed to the regexp engine has to be
	// the same 25 characters IsSpace answers for.
	for character := rune(0); character <= 0xFFFF; character++ {
		inClass := strings.Contains(js.SpaceClass, fmt.Sprintf("\\x{%04X}", character))
		if inClass != js.IsSpace(character) {
			failures += fail("U+%04X is %v in js.SpaceClass and %v in js.IsSpace", character, inClass, js.IsSpace(character))
		}
	}

	fmt.Println("numbers      rule 3 at its edges, and the whitespace class against IsSpace")
	return failures
}

func positiveInfinity() float64 { return 1 / zero() }
func negativeInfinity() float64 { return -1 / zero() }
func notANumber() float64       { return zero() / zero() }
func zero() float64             { return 0 }
