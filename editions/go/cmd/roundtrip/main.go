// Prove the canonical writer against the oracle, before anything is built on it.
//
//	go run ./cmd/roundtrip [--corpus-root corpus]
//
// Every `expected` string in the corpus is parsed by internal/canonical and re-encoded by it, and
// the bytes must come back identical. That is a stronger statement than it looks: those strings
// were produced by `reference/typescript/canonical.ts` from real contract output, so they carry
// every number shape, key ordering and escape the system actually emits — thousands of them, none
// of them chosen by the person writing this edition.
//
// It exists because of finding 155, recorded when the C++ edition was written:
//
//	Prove the serialiser against the oracle before building anything on it. A canonical writer is
//	the one component every case in the corpus passes through, and 1,148 round-trips cost an hour
//	and dictated the object container's design. Found after twenty contracts, it is twenty
//	rewrites.
//
// A failure here is not a contract bug. It is the writer disagreeing with the reference about how
// a value is spelt, and it would otherwise present as some unrelated contract failing three
// hundred cases later with a diff nobody could read.
package main

import (
	"flag"
	"fmt"
	"os"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/corpus"
)

func main() {
	root := flag.String("corpus-root", "", "the corpus directory (default: SKYNET_CORPUS, or found above the working directory)")
	verbose := flag.Bool("verbose", false, "print every mismatch rather than the first ten")
	flag.Parse()

	if err := run(*root, *verbose); err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		os.Exit(1)
	}
}

func run(explicitRoot string, verbose bool) error {
	root, err := corpus.Root(explicitRoot)
	if err != nil {
		return err
	}
	files, err := corpus.Load(root)
	if err != nil {
		return err
	}
	if len(files) == 0 {
		return fmt.Errorf("no corpus files under %q", root)
	}

	checked, failed := 0, 0
	shown := 0
	for _, file := range files {
		for _, vector := range file.Cases {
			checked++
			parsed, err := canonical.Decode(vector.Expected)
			if err != nil {
				failed++
				shown = report(shown, verbose, "%s :: %s\n  did not parse: %v", file.Key(), vector.ID, err)
				continue
			}
			written, err := canonical.Encode(parsed)
			if err != nil {
				failed++
				shown = report(shown, verbose, "%s :: %s\n  did not re-encode: %v", file.Key(), vector.ID, err)
				continue
			}
			if written != vector.Expected {
				failed++
				shown = report(shown, verbose, "%s :: %s\n  expected %s\n  wrote    %s\n  %s",
					file.Key(), vector.ID, vector.Expected, written, firstDifference(vector.Expected, written))
			}
		}
	}

	fmt.Printf("round trip  %d checked, %d failed, across %d files\n", checked, failed, len(files))
	if failed != 0 {
		return fmt.Errorf("the canonical writer does not reproduce the oracle's bytes")
	}
	return nil
}

// report prints a failure, capping the output at ten unless asked for all of them. A run that
// printed nine hundred diffs would bury the first one, which is the only one anybody reads.
func report(shown int, verbose bool, format string, args ...any) int {
	if !verbose && shown >= 10 {
		return shown + 1
	}
	if !verbose && shown == 9 {
		fmt.Printf(format+"\n\n... and more (pass --verbose)\n\n", args...)
		return shown + 1
	}
	fmt.Printf(format+"\n\n", args...)
	return shown + 1
}

// firstDifference names the byte the two strings part company at, because two nearly-identical
// four-kilobyte JSON documents are not diffable by eye.
func firstDifference(expected, written string) string {
	limit := len(expected)
	if len(written) < limit {
		limit = len(written)
	}
	for at := 0; at < limit; at++ {
		if expected[at] != written[at] {
			return fmt.Sprintf("first difference at byte %d: expected %q, wrote %q",
				at, window(expected, at), window(written, at))
		}
	}
	return fmt.Sprintf("identical for %d bytes, then one is longer", limit)
}

func window(text string, at int) string {
	start := at - 20
	if start < 0 {
		start = 0
	}
	end := at + 20
	if end > len(text) {
		end = len(text)
	}
	return text[start:end]
}
