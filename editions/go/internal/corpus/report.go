package corpus

import (
	"fmt"
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
)

// Handler computes one case's output from its input.
//
// It returns the value the contract produced; the runner encodes it and compares bytes. An error
// is a case this edition could not answer *at all* — not a case it answered wrongly. The two are
// counted separately and neither is allowed to hide in the other.
type Handler func(input canonical.Value) (canonical.Value, error)

// Dispatch routes a corpus file to the contract that answers it.
//
// Keyed by `corpus/file` rather than by corpus, because a corpus is answered file by file:
// `transport/scaling` can be written while `transport/retarget` is not, and a table keyed by
// corpus would force an edition to claim all seven at once or none.
type Dispatch map[string]Handler

// Failure is one case that was answered and answered wrongly.
type Failure struct {
	File     string
	Case     string
	Note     string
	Expected string
	Actual   string
	Err      error
}

// Describe renders a failure for a human, which means saying which case, why it exists, what was
// expected, what came out, and where the two part company. A diff of two four-kilobyte JSON
// documents is not readable without that last line.
func (f Failure) Describe() string {
	var out strings.Builder
	fmt.Fprintf(&out, "%s :: %s\n  %s\n", f.File, f.Case, f.Note)
	if f.Err != nil {
		fmt.Fprintf(&out, "  error: %v", f.Err)
		return out.String()
	}
	fmt.Fprintf(&out, "  expected %s\n  actual   %s\n  %s", f.Expected, f.Actual, firstDifference(f.Expected, f.Actual))
	return out.String()
}

func firstDifference(expected, actual string) string {
	limit := len(expected)
	if len(actual) < limit {
		limit = len(actual)
	}
	for at := 0; at < limit; at++ {
		if expected[at] != actual[at] {
			return fmt.Sprintf("first difference at byte %d: expected %q, got %q",
				at, window(expected, at), window(actual, at))
		}
	}
	if len(expected) == len(actual) {
		return "identical"
	}
	return fmt.Sprintf("identical for %d bytes, then one is longer", limit)
}

func window(text string, at int) string {
	start := at - 24
	if start < 0 {
		start = 0
	}
	end := at + 24
	if end > len(text) {
		end = len(text)
	}
	return text[start:end]
}

// Report is what a run of the corpus produced.
//
// Four buckets, and the design rule behind them is that none of the four is useful without the
// others. §31.4 says an edition that has not passed the corpus is not an edition, so "8 of 33
// corpora, all passing" has to be as visible as the zero failures, and a corpus this edition
// answers *part* of has to be visibly neither complete nor missing.
type Report struct {
	Passed          int
	Failed          int
	Unanswered      int
	Complete        []string
	Partial         []string
	MissingHandlers []string
	DeclinedFiles   []string
	Failures        []Failure
}

// Total is every case in the corpus, answered or not.
func (r Report) Total() int { return r.Passed + r.Failed + r.Unanswered }

// Summary is the one line a human reads first.
func (r Report) Summary() string {
	return fmt.Sprintf("%d passed, %d failed, %d not implemented, of %d",
		r.Passed, r.Failed, r.Unanswered, r.Total())
}

// Run measures a dispatch against the corpus.
//
// A handler that panics is caught and recorded as a failure of its case rather than taking the
// runner down. This is not leniency about panics — every one of them is a bug in this edition and
// is reported as a red case — it is finding 164: *a runner that crashes describing a failure hides
// the failure*. An index out of range in one contract must not cost the other thirty-two corpora
// their result.
func Run(files []File, dispatch Dispatch) Report {
	report := Report{}
	answeredByCorpus := map[string]int{}
	filesByCorpus := map[string]int{}
	handledByCorpus := map[string]int{}

	for _, file := range files {
		filesByCorpus[file.Corpus]++
		handler, found := dispatch[file.Key()]
		if !found {
			report.Unanswered += len(file.Cases)
			report.DeclinedFiles = append(report.DeclinedFiles, file.Key())
			continue
		}
		handledByCorpus[file.Corpus]++
		answeredByCorpus[file.Corpus]++
		for _, vector := range file.Cases {
			actual, err := call(handler, vector.Input)
			if err != nil {
				report.Failed++
				report.Failures = append(report.Failures, Failure{
					File: file.Key(), Case: vector.ID, Note: vector.Note, Err: err,
				})
				continue
			}
			encoded, err := canonical.Encode(actual)
			if err != nil {
				report.Failed++
				report.Failures = append(report.Failures, Failure{
					File: file.Key(), Case: vector.ID, Note: vector.Note, Err: err,
				})
				continue
			}
			if encoded != vector.Expected {
				report.Failed++
				report.Failures = append(report.Failures, Failure{
					File: file.Key(), Case: vector.ID, Note: vector.Note,
					Expected: vector.Expected, Actual: encoded,
				})
				continue
			}
			report.Passed++
		}
	}

	failedCorpora := map[string]bool{}
	for _, failure := range report.Failures {
		failedCorpora[strings.SplitN(failure.File, "/", 2)[0]] = true
	}
	for corpusName, total := range filesByCorpus {
		switch {
		case handledByCorpus[corpusName] == 0:
			report.MissingHandlers = append(report.MissingHandlers, corpusName)
		case handledByCorpus[corpusName] < total || failedCorpora[corpusName]:
			report.Partial = append(report.Partial, corpusName)
		default:
			report.Complete = append(report.Complete, corpusName)
		}
	}
	// `DeclinedFiles` holds the files of corpora that are partly answered; a corpus with no handler
	// at all is named once in MissingHandlers rather than once per file.
	report.DeclinedFiles = filterDeclined(report.DeclinedFiles, report.MissingHandlers)

	sort.Strings(report.Complete)
	sort.Strings(report.Partial)
	sort.Strings(report.MissingHandlers)
	sort.Strings(report.DeclinedFiles)
	return report
}

func filterDeclined(declined []string, missing []string) []string {
	absent := map[string]bool{}
	for _, corpusName := range missing {
		absent[corpusName] = true
	}
	kept := declined[:0]
	for _, key := range declined {
		if !absent[strings.SplitN(key, "/", 2)[0]] {
			kept = append(kept, key)
		}
	}
	return kept
}

func call(handler Handler, input canonical.Value) (value canonical.Value, err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			value = canonical.Null()
			err = fmt.Errorf("the handler panicked: %v", recovered)
		}
	}()
	return handler(input)
}
