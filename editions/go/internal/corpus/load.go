// Loading the conformance corpus.
//
// The corpus is plain data — corpus/README.md's first property is that `expected` is a *string*,
// not an object, because the requirement is byte-identical output and the bytes are what the file
// holds. So nothing here interprets a case; it reads files, and the comparison a runner makes is
// between two strings.
package corpus

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
)

// Case is one vector: an input, the bytes the reference produced from it, and the note that says
// what it is for.
type Case struct {
	ID       string
	Note     string
	Input    canonical.Value
	Expected string
}

// File is one corpus file — `corpus/transport/scaling.json` and its cases.
type File struct {
	// Corpus is the directory name: `transport`, `cycle`, `roles`. Several files share one.
	Corpus string
	// Name is the file's stem: `scaling`, `binding`. Corpus and Name together are what a dispatch
	// keys on and what a failure is reported under.
	Name  string
	Cases []Case
}

// Key is the `corpus/name` a dispatch is registered under and a failure is printed as.
func (f File) Key() string { return f.Corpus + "/" + f.Name }

// Root finds the corpus directory.
//
// Three sources, in order, and each is there for a caller that exists: an explicit path from the
// command line, SKYNET_CORPUS for a runner invoked from somewhere else in a build, and a walk up
// from the working directory for the ordinary case of running inside the repository. The walk
// stops at the filesystem root and returns an error rather than defaulting to something plausible,
// because a runner that silently measured an empty corpus would report zero failures.
func Root(explicit string) (string, error) {
	if explicit != "" {
		return verify(explicit)
	}
	if fromEnvironment := os.Getenv("SKYNET_CORPUS"); fromEnvironment != "" {
		return verify(fromEnvironment)
	}
	here, err := os.Getwd()
	if err != nil {
		return "", fmt.Errorf("no working directory: %w", err)
	}
	for {
		candidate := filepath.Join(here, "corpus")
		if info, err := os.Stat(filepath.Join(candidate, "README.md")); err == nil && !info.IsDir() {
			return candidate, nil
		}
		parent := filepath.Dir(here)
		if parent == here {
			return "", fmt.Errorf("no corpus found above %q: pass --corpus-root, or set SKYNET_CORPUS", here)
		}
		here = parent
	}
}

func verify(path string) (string, error) {
	info, err := os.Stat(path)
	if err != nil {
		return "", fmt.Errorf("corpus at %q: %w", path, err)
	}
	if !info.IsDir() {
		return "", fmt.Errorf("corpus at %q is not a directory", path)
	}
	return path, nil
}

// Load reads every corpus file under root, in a fixed order.
//
// Sorted by corpus and then by file name, so two runs of the same runner print their failures in
// the same order and a diff of two reports is a diff of the results rather than of the directory
// listing.
func Load(root string) ([]File, error) {
	directories, err := os.ReadDir(root)
	if err != nil {
		return nil, fmt.Errorf("reading %q: %w", root, err)
	}
	var files []File
	for _, directory := range directories {
		if !directory.IsDir() {
			continue
		}
		entries, err := os.ReadDir(filepath.Join(root, directory.Name()))
		if err != nil {
			return nil, fmt.Errorf("reading %q: %w", directory.Name(), err)
		}
		for _, entry := range entries {
			if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
				continue
			}
			file, err := loadFile(filepath.Join(root, directory.Name(), entry.Name()), directory.Name())
			if err != nil {
				return nil, err
			}
			files = append(files, file)
		}
	}
	sort.Slice(files, func(left, right int) bool { return files[left].Key() < files[right].Key() })
	return files, nil
}

func loadFile(path string, corpusName string) (File, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return File{}, fmt.Errorf("reading %q: %w", path, err)
	}
	document, err := canonical.Decode(string(raw))
	if err != nil {
		return File{}, fmt.Errorf("parsing %q: %w", path, err)
	}
	name := strings.TrimSuffix(filepath.Base(path), ".json")

	// The corpus names itself in a field, and a file whose directory and field disagree is a file
	// that was moved without being edited. Checked rather than trusted: the dispatch keys on the
	// directory, and a silent mismatch would route a case to the wrong contract.
	if declared, ok := document.Field("corpus").Str(); !ok || declared != corpusName {
		return File{}, fmt.Errorf("%q declares corpus %q but sits in %q", path, declared, corpusName)
	}

	cases := document.Field("cases")
	if cases.Kind() != canonical.KindArray {
		return File{}, fmt.Errorf("%q has no `cases` array", path)
	}
	file := File{Corpus: corpusName, Name: name}
	for at, entry := range cases.Items() {
		id, ok := entry.Field("id").Str()
		if !ok {
			return File{}, fmt.Errorf("%q case %d has no `id`", path, at)
		}
		expected, ok := entry.Field("expected").Str()
		if !ok {
			return File{}, fmt.Errorf("%q case %q has no `expected` string", path, id)
		}
		// The note is required by the corpus's third property — "a vector whose purpose nobody
		// recorded is a vector nobody dares change when it fails" — and a runner that shrugged at a
		// missing one would be the reason the next case ships without it.
		note, ok := entry.Field("note").Str()
		if !ok {
			return File{}, fmt.Errorf("%q case %q has no `note`", path, id)
		}
		file.Cases = append(file.Cases, Case{
			ID:       id,
			Note:     note,
			Input:    entry.Field("input"),
			Expected: expected,
		})
	}
	return file, nil
}
