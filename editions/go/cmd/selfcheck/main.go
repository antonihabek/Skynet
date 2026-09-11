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
//
//  4. **The compiler's own output**, on the architectures this machine is not. See fusionScan: the
//     one divergence in this edition that no run of the corpus on an amd64 host can ever show.
package main

import (
	"context"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/corpus"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

func main() {
	failures := 0
	failures += sourceScan()
	failures += runnerAccounting()
	failures += numberEdges()
	failures += fusionScan()

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

// typographicQuotes are what gofmt leaves behind when it rewrites a doc comment.
//
// Since Go 1.19 gofmt reformats doc comments, and one of its rewrites turns a pair of apostrophes
// into a closing double quotation mark — so a comment quoting the reference's empty-string default
// comes out quoting something that is neither the reference nor English. (This comment cannot show
// the pair without gofmt rewriting it too, which is the whole problem.) It happened five times in
// this edition before anything looked, each one a comment that then misquoted the line it
// explained. The rewrite leaves no other trace, and this edition's prose uses straight quotes
// everywhere, so a curly one is the signal.
var typographicQuotes = []string{"“", "”"}

func scanFile(path string) int {
	fileSet := token.NewFileSet()
	parsed, err := parser.ParseFile(fileSet, path, nil, parser.ParseComments)
	if err != nil {
		return fail("%s: %v", path, err)
	}
	failures := 0
	for _, group := range parsed.Comments {
		for _, comment := range group.List {
			for _, quote := range typographicQuotes {
				if strings.Contains(comment.Text, quote) {
					position := fileSet.Position(comment.Pos())
					failures += fail("%s:%d: a typographic quote in a comment — gofmt rewrites `''` in a doc "+
						"comment into one; write the empty string as \"\" or say it in words", path, position.Line)
				}
			}
		}
	}
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

// ── 4. the compiler's own output ────────────────────────────────────────────

// fusingTargets are the architectures on which the Go compiler fuses a multiply into an add.
//
// The Go specification permits it in so many words — "an implementation may combine multiple
// floating-point operations into a single fused operation, possibly across statements, and produce
// a result that differs from the value obtained by executing and rounding the instructions
// individually" — and the reference forbids it just as plainly: `kinematics.ts` says *nothing here
// uses a fused multiply-add*, because §31.2's argument for writing sine and cosine out rests on the
// same sequence of correctly rounded operations producing the same bits everywhere. A fused
// operation is a different sequence.
//
// The list is measured, not remembered: each of the six fuses `x*y + z` into one instruction, and
// mips64, 386 and wasm fuse nothing. amd64 is here at v3 only; at its default level there is no FMA
// instruction to fuse into, which is why every corpus run on a developer's x86 machine passes and
// why this check has to compile for machines other than the one it runs on. A Jetson is arm64.
var fusingTargets = []struct {
	goarch string
	level  string
}{
	{"arm64", ""},
	{"amd64", "v3"},
	{"riscv64", ""},
	{"ppc64le", ""},
	{"s390x", ""},
	{"loong64", ""},
}

// assemblyLine is one instruction in the compiler's -S listing: the source position, then the
// mnemonic. The position is what makes a finding actionable — a fused instruction is reported
// against the line of Go that produced it, which is the line that needs a float64 conversion.
var assemblyLine = regexp.MustCompile(`\(([^()]+\.go):([0-9]+)\)\t([A-Z][A-Z0-9.]*)`)

// fusedMnemonic is every spelling of a fused multiply-add the six targets emit: FMADDD, FMSUBD,
// FNMADDD and FNMSUBD on arm64, riscv64 and loong64; FMADD and its three siblings on ppc64 and
// s390x; VFMADD231SD and the rest of the VEX family on amd64.
var fusedMnemonic = regexp.MustCompile(`^V?FN?M(ADD|SUB)[0-9A-Z]*$`)

// probeFile is the positive control's path, relative to the edition.
const probeFile = "internal/fusionprobe/probe.go"

// compileTimeout bounds one architecture's build. A cold cache compiles the standard library for
// that architecture first, which takes a minute or two; a warm one replays the listing in under a
// second.
const compileTimeout = 10 * time.Minute

// fusionScan refuses any fused multiply-add in the compiled edition, on every architecture that has
// one.
//
// It reads the compiler's output rather than the source, and that is the whole of the design. The
// fusion is a property of the SSA backend, not of the syntax: `p := x * y` on one line and `p + z` on
// the next fuse exactly as `x*y + z` does, and so does a product passed into a helper that the
// compiler inlines and then subtracts from. Both were measured before this was written, and the
// second is the defect this check found on its first run — `micros` handing `ms * 1000` to
// roundHalfAway, which computed the fraction from the unrounded product on arm64. No source scan
// sees across an inlining boundary. The listing does.
//
// The remedy the specification gives is an explicit conversion — "an explicit floating-point type
// conversion rounds to the precision of the target type, preventing fusion" — so a finding here is
// fixed by writing float64(x*y) where the product is formed, or float64(v) where a helper receives
// one.
func fusionScan() int {
	failures := 0
	for _, target := range fusingTargets {
		name := target.goarch
		if target.level != "" {
			name += " " + target.level
		}

		listing, err := compileAssembly(target.goarch, target.level)
		if err != nil {
			failures += fail("fusion scan (%s): %v", name, err)
			continue
		}

		instructions := 0
		probeFused := false
		fused := map[string]string{}
		for _, line := range strings.Split(listing, "\n") {
			match := assemblyLine.FindStringSubmatch(line)
			if match == nil {
				continue
			}
			instructions++
			if !fusedMnemonic.MatchString(match[3]) {
				continue
			}
			path := editionRelative(match[1])
			if path == probeFile {
				probeFused = true
				continue
			}
			fused[path+":"+match[2]] = match[3]
		}

		// Three ways to pass by not looking, and each is refused by name.
		if instructions == 0 {
			failures += fail("fusion scan (%s): the build printed no assembly, so nothing was checked", name)
			continue
		}
		if !probeFused {
			failures += fail("fusion scan (%s): the probe in %s did not fuse, so the scan cannot see a "+
				"fused instruction on this target — a mnemonic the pattern does not know, or a compiler "+
				"that stopped fusing; either way the result below would mean nothing", name, probeFile)
			continue
		}
		sites := make([]string, 0, len(fused))
		for site := range fused {
			sites = append(sites, site)
		}
		sort.Strings(sites)
		for _, site := range sites {
			failures += fail("%s: %s on %s — a product fused into an add; round it with float64(…) "+
				"where it is formed, or where the helper that inlines it receives it", site, fused[site], name)
		}
	}
	fmt.Printf("fusion       %d architectures, each with its positive control\n", len(fusingTargets))
	return failures
}

// compileAssembly builds the edition's internal packages for one architecture and returns the
// compiler's listing. Linux as the operating system for every target, because the question is the
// instruction set and not the platform; CGO off, because nothing here uses it and a cross build with
// it on would need a C compiler for each target.
func compileAssembly(goarch, level string) (string, error) {
	deadline, cancel := context.WithTimeout(context.Background(), compileTimeout)
	defer cancel()

	command := exec.CommandContext(deadline, "go", "build", "-gcflags=-S", "./internal/...")
	command.Env = append(os.Environ(), "GOOS=linux", "GOARCH="+goarch, "CGO_ENABLED=0")
	if level != "" {
		command.Env = append(command.Env, "GOAMD64="+level)
	}
	output, err := command.CombinedOutput()
	if deadline.Err() != nil {
		return "", fmt.Errorf("the build did not finish inside %v", compileTimeout)
	}
	if err != nil {
		return "", fmt.Errorf("the build failed: %v\n%s", err, lastLines(string(output), 20))
	}
	return string(output), nil
}

// editionRelative turns the compiler's absolute source path into the path this edition's other
// checks print, so a finding reads the same whichever machine produced it.
func editionRelative(path string) string {
	slashed := filepath.ToSlash(path)
	if at := strings.Index(slashed, "/editions/go/"); at >= 0 {
		return slashed[at+len("/editions/go/"):]
	}
	return slashed
}

func lastLines(text string, count int) string {
	lines := strings.Split(strings.TrimSuffix(text, "\n"), "\n")
	if len(lines) > count {
		lines = lines[len(lines)-count:]
	}
	return strings.Join(lines, "\n")
}

func positiveInfinity() float64 { return 1 / zero() }
func negativeInfinity() float64 { return -1 / zero() }
func notANumber() float64       { return zero() / zero() }
func zero() float64             { return 0 }
