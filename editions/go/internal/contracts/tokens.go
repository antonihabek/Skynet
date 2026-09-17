// §85's capability token vocabulary: what a string in a capability list is.
//
// Written from `classifyToken` in `reference/typescript/package.ts`. The check is byte-wise on
// purpose: `unicode.IsLower` and `unicode.IsDigit` accept letters and digits outside ASCII, which the
// reference's code-unit comparison does not, and the self-check's source scan refuses them anyway.
package contracts

import (
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// wellFormedToken is two or more dot-separated segments, each an ASCII lowercase letter followed by
// ASCII lowercase letters, digits or underscores.
func wellFormedToken(token string) bool {
	segments := strings.Split(token, ".")
	if len(segments) < 2 {
		return false
	}
	for _, segment := range segments {
		if segment == "" || segment[0] < 'a' || segment[0] > 'z' {
			return false
		}
		for i := 1; i < len(segment); i++ {
			c := segment[i]
			if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '_') {
				return false
			}
		}
	}
	return true
}

// ClassifyToken is §85.1's four classes. Exact: nothing trimmed, nothing folded, and membership in
// the list is the whole test — a canonical domain does not admit a token by shape.
func ClassifyToken(token string) string {
	if !wellFormedToken(token) {
		return "malformed"
	}
	if vocabulary.Contains(vocabulary.CapabilityTokens, token) {
		return "canonical"
	}
	if vocabulary.Contains(vocabulary.CapabilityDomains, token[:strings.Index(token, ".")]) {
		return "unknown"
	}
	return "vendor"
}

// distinct is the tokens in first-appearance order, each once.
func distinct(tokens []string) []string {
	out := []string{}
	for _, token := range tokens {
		if !containsString(out, token) {
			out = append(out, token)
		}
	}
	return out
}

func admissionTokens(input canonical.Value) (canonical.Value, error) {
	token, _ := input.Field("token").Str()
	return canonical.Object().Set("class", canonical.String(ClassifyToken(token))), nil
}
