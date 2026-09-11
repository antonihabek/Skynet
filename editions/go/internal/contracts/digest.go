package contracts

import (
	"crypto/sha256"
	"encoding/hex"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
)

// Version digests.
//
//	sha256(
//	  "skynet.version.v1" \n
//	  kind                \n
//	  version             \n
//	  canonical(manifest) \n
//	  "<path>\0<sha256>" \n … per file, sorted by path
//	)
//
// The separator between a path and its hash is a NUL rather than a space: it is the one byte a path
// may not contain, which checkPath enforces, and that is what makes the framing unambiguous rather
// than merely unlikely to collide. Written here as an escape rather than as the byte itself,
// because the reference records that its own implementation was first written against a space and
// found out from a digest that did not match — a terminal renders the raw byte as nothing.
//
// There is one deliberate restriction, and it is the reason `scan` exists: a manifest carrying a
// number at or above 1e21 or below 1e-6 is **refused**, and so is one whose object *keys* carry a
// character above U+FFFF. Both are places where the Hub's serialiser and §31.4's canonical rule can
// disagree — an exponent in the first case, UTF-16 versus UTF-8 key order in the second — and
// refusal is the same discipline the rest of the platform applies to a missing envelope: a package
// that does not install and a person reading a reason beats a package whose identity is one thing
// on the robot and another in the registry.

// VersionDigestDomain is present so a digest over a version cannot collide with a digest over
// anything else.
const VersionDigestDomain = "skynet.version.v1"

const (
	jsMinPlain = 1e-6
	jsMaxPlain = 1e21
)

// DigestFile is one file of a package: a path relative to the root, and the lowercase hex sha256 of
// its bytes.
type DigestFile struct {
	Path   string
	SHA256 string
}

// DigestError is why a digest could not be computed. Each names the input rather than saying
// "failed".
type DigestError struct {
	Kind       string
	Subject    string
	HasSubject bool
	Detail     string
	HasDetail  bool
}

var hexSha256Pattern = regexp.MustCompile(`^[0-9a-f]{64}$`)

// IsHexSHA256 reports whether a string is a lowercase hex sha256.
func IsHexSHA256(value string) bool { return hexSha256Pattern.MatchString(value) }

// CheckPath reports why a relative path may not be used as one, or "" when it may.
//
// The file list is both what is hashed and what is written to disk, and every rule here closes a
// way for the second to leave the package's own directory or to name two things on two filesystems.
// Checked before hashing rather than at unpack time, because a digest computed over a traversal is
// a digest that attests to one.
func CheckPath(path string) string {
	if len(path) == 0 {
		return "empty"
	}
	// Bytes, not characters: the reference asks Buffer.byteLength in UTF-8, and len() on a Go
	// string is exactly that.
	if len(path) > 1024 {
		return "longer than 1024 bytes"
	}
	if strings.HasPrefix(path, "/") {
		return "absolute; paths are relative to the package root"
	}
	if strings.Contains(path, `\`) {
		return "contains a backslash, which is a separator on one filesystem and an ordinary filename character on another"
	}
	for _, character := range path {
		if character < 0x20 || character == 0x7f {
			return "contains a control character"
		}
	}
	for _, component := range strings.Split(path, "/") {
		switch component {
		case "":
			return "has an empty component, so two spellings name one file"
		case ".":
			return "has a `.` component"
		case "..":
			return "has a `..` component, which leaves the package root"
		}
		if strings.HasSuffix(component, " ") || strings.HasSuffix(component, ".") {
			return "has a component ending in a space or a dot, which some filesystems silently strip"
		}
	}
	return ""
}

// utf16Less compares two strings by UTF-16 code unit, which is what JavaScript's `<` does.
//
// Go's `<` is a UTF-8 byte comparison, and the two disagree: in UTF-16 an astral character is a
// surrogate pair beginning at U+D800, so it sorts *before* U+E000..U+FFFF, while its UTF-8 encoding
// begins with 0xF0 and sorts *after*. Every path in the basic multilingual plane orders the same
// either way, which is why an edition using Go's `<` would agree with the reference on every file
// list anybody has and disagree on the first one carrying an emoji — and the disagreement would be
// a digest, not an error.
func utf16Less(left, right string) bool {
	leftUnits, rightUnits := utf16Units(left), utf16Units(right)
	shared := len(leftUnits)
	if len(rightUnits) < shared {
		shared = len(rightUnits)
	}
	for at := 0; at < shared; at++ {
		if leftUnits[at] != rightUnits[at] {
			return leftUnits[at] < rightUnits[at]
		}
	}
	return len(leftUnits) < len(rightUnits)
}

func utf16Units(text string) []uint16 {
	units := make([]uint16, 0, len(text))
	for _, character := range text {
		if character > 0xffff {
			character -= 0x10000
			units = append(units, uint16(0xd800+(character>>10)), uint16(0xdc00+(character&0x3ff)))
			continue
		}
		units = append(units, uint16(character))
	}
	return units
}

// scan walks a manifest for the two things that would make its identity ambiguous.
func scan(value canonical.Value, at string) (DigestError, bool) {
	switch value.Kind() {
	case canonical.KindNumber:
		number, _ := value.Number()
		magnitude := math.Abs(number)
		diverges := !canonical.Finite(number) || magnitude >= jsMaxPlain || (magnitude != 0 && magnitude < jsMinPlain)
		if diverges {
			return DigestError{
				Kind: "number_out_of_range", Subject: fieldName(at), HasSubject: true,
			}, true
		}
		return DigestError{}, false
	case canonical.KindArray:
		for index, item := range value.Items() {
			if found, bad := scan(item, at+"["+strconv.Itoa(index)+"]"); bad {
				return found, true
			}
		}
		return DigestError{}, false
	case canonical.KindObject:
		for _, member := range value.Members() {
			for _, character := range member.Key {
				if character > 0xffff {
					return DigestError{
						Kind: "key_outside_bmp", Subject: fieldName(at) + " :: " + member.Key, HasSubject: true,
					}, true
				}
			}
			next := member.Key
			if at != "" {
				next = at + "." + member.Key
			}
			if found, bad := scan(member.Value, next); bad {
				return found, true
			}
		}
	}
	return DigestError{}, false
}

func fieldName(at string) string {
	if at == "" {
		return "(root)"
	}
	return at
}

// VersionDigest computes the digest of a version, or says precisely why it cannot be computed.
func VersionDigest(kind, version string, manifest canonical.Value, files []DigestFile) (string, DigestError, bool) {
	if bad, found := scan(manifest, ""); found {
		return "", bad, false
	}

	for _, file := range files {
		if detail := CheckPath(file.Path); detail != "" {
			return "", DigestError{
				Kind: "path", Subject: file.Path, HasSubject: true, Detail: detail, HasDetail: true,
			}, false
		}
		if !IsHexSHA256(file.SHA256) {
			return "", DigestError{Kind: "sha256", Subject: file.Path, HasSubject: true}, false
		}
	}

	sorted := make([]DigestFile, len(files))
	copy(sorted, files)
	// Stable, because the reference's Array.prototype.sort is stable and two files with equal paths
	// reach the duplicate check below in the order they were given.
	sort.SliceStable(sorted, func(left, right int) bool {
		return utf16Less(sorted[left].Path, sorted[right].Path)
	})
	for at := 1; at < len(sorted); at++ {
		if sorted[at].Path == sorted[at-1].Path {
			return "", DigestError{Kind: "duplicate_path", Subject: sorted[at].Path, HasSubject: true}, false
		}
	}

	manifestText, err := canonical.Encode(manifest)
	if err != nil {
		// The reference catches the serialiser throwing and reports `manifest` with the message.
		// Unreachable from here — scan has already refused every non-finite number, which is the
		// only thing Encode refuses — and handled rather than assumed away.
		return "", DigestError{Kind: "manifest", Detail: err.Error(), HasDetail: true}, false
	}

	entries := make([]string, 0, len(sorted))
	for _, file := range sorted {
		entries = append(entries, file.Path+"\x00"+file.SHA256)
	}
	body := strings.Join([]string{
		VersionDigestDomain,
		kind,
		version,
		manifestText,
		strings.Join(entries, "\n"),
	}, "\n")

	sum := sha256.Sum256([]byte(body))
	return hex.EncodeToString(sum[:]), DigestError{}, true
}

// ── the corpus ──────────────────────────────────────────────────────────────

func versionDigest(input canonical.Value) (canonical.Value, error) {
	kind, _ := input.Field("kind").Str()
	version, _ := input.Field("version").Str()
	manifest := input.Field("manifest")
	if manifest.IsNull() {
		// The reference writes `params.manifest ?? {}` — twice, and both have to be the same
		// object or the scan and the hash would disagree about what was checked.
		manifest = canonical.Object()
	}

	var files []DigestFile
	for _, entry := range input.Field("files").Items() {
		path, _ := entry.Field("path").Str()
		digest, _ := entry.Field("sha256").Str()
		files = append(files, DigestFile{Path: path, SHA256: digest})
	}

	digest, failure, ok := VersionDigest(kind, version, manifest, files)
	if ok {
		return canonical.Object().
			Set("digest", canonical.String(digest)).
			Set("error", canonical.Null()), nil
	}
	return canonical.Object().
		Set("digest", canonical.Null()).
		Set("error", canonical.Object().
			Set("kind", canonical.String(failure.Kind)).
			Set("subject", optionalString(failure.Subject, failure.HasSubject)).
			Set("detail", optionalString(failure.Detail, failure.HasDetail))), nil
}
