// Run the conformance corpus against the Go edition.
//
//	skynet-corpus-go [--corpus-root corpus] [--verbose] [--json] [--min-passed N]
//
// Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary
// always says how many cases are not implemented — split into the corpora with no handler at all
// and the individual files a handler has not been written for. Those facts together are the honest
// sentence about this edition, and none of them is useful without the others: §31.4 says an
// edition that has not passed the corpus is not an edition, so "8 of 33 corpora, all passing" has
// to be as visible as the zero failures.
//
// `--min-passed` is the ratchet, and it exists because everything above is a **floor of zero**. An
// edition whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move
// from *passed* into *not implemented*, which is the bucket that reads as honest progress. That is
// the one regression this runner's own design cannot see, so the number a CI step passes here is
// the number this edition has reached, and lowering it is a decision somebody makes in a diff.
package main

import (
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/contracts"
	"github.com/antonihabek/Skynet/editions/go/internal/corpus"
)

func main() {
	root := flag.String("corpus-root", "", "the corpus directory (default: SKYNET_CORPUS, or found above the working directory)")
	verbose := flag.Bool("verbose", false, "print every failure rather than the first ten")
	asJSON := flag.Bool("json", false, "print the report as canonical JSON, for tools/edition-report.mjs")
	minPassed := flag.Int("min-passed", -1, "fail if fewer than this many cases passed (the ratchet)")
	flag.Parse()

	code, err := run(*root, *verbose, *asJSON, *minPassed)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(2)
	}
	os.Exit(code)
}

func run(explicitRoot string, verbose, asJSON bool, minPassed int) (int, error) {
	root, err := corpus.Root(explicitRoot)
	if err != nil {
		return 0, err
	}
	files, err := corpus.Load(root)
	if err != nil {
		return 0, err
	}
	if len(files) == 0 {
		return 0, fmt.Errorf("no corpus files under %q", root)
	}

	report := corpus.Run(files, contracts.Dispatch())

	if asJSON {
		// Deliberately the same fields the human report prints, and no derived ones: a summary
		// that computed a percentage here and a page that computed it differently would be two
		// answers to one question.
		out := canonical.Object().
			Set("passed", canonical.Number(float64(report.Passed))).
			Set("failed", canonical.Number(float64(report.Failed))).
			Set("unimplemented", canonical.Number(float64(report.Unanswered))).
			Set("total", canonical.Number(float64(report.Total()))).
			Set("complete", canonical.Strings(report.Complete)).
			Set("partial", canonical.Strings(report.Partial)).
			Set("missing_handlers", canonical.Strings(report.MissingHandlers)).
			Set("declined_files", canonical.Strings(report.DeclinedFiles))
		encoded, err := canonical.Encode(out)
		if err != nil {
			return 0, err
		}
		fmt.Println(encoded)
		if report.Failed != 0 {
			return 1, nil
		}
		return 0, nil
	}

	fmt.Printf("corpus     %s\n", root)
	fmt.Printf("result     %s\n", report.Summary())
	printList("complete  ", report.Complete)
	printList("no handler", report.MissingHandlers)
	printList("declined  ", report.DeclinedFiles)
	// Named separately from `complete` on purpose: these are corpora this edition answers part of,
	// and a reader who saw only the passing files would take them for finished ones.
	printList("partial   ", report.Partial)

	if len(report.Failures) > 0 {
		fmt.Println()
		shown := len(report.Failures)
		if !verbose && shown > 10 {
			shown = 10
		}
		for _, failure := range report.Failures[:shown] {
			fmt.Printf("%s\n\n", failure.Describe())
		}
		if shown < len(report.Failures) {
			fmt.Printf("... and %d more (pass --verbose)\n", len(report.Failures)-shown)
		}
	}

	if minPassed >= 0 && report.Passed < minPassed {
		fmt.Printf("\nregression: %d passed, and this edition had reached %d. Cases that stop being "+
			"answered are counted as *not implemented* rather than as failures, so nothing above "+
			"this line went red — which is why the floor is checked separately from the failures.\n",
			report.Passed, minPassed)
		return 1, nil
	}
	if report.Failed != 0 {
		return 1, nil
	}
	return 0, nil
}

func printList(heading string, values []string) {
	if len(values) == 0 {
		return
	}
	fmt.Printf("%s %s\n", heading, strings.Join(values, ", "))
}
