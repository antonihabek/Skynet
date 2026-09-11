package js

import (
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
)

// ToString is ECMAScript's ToString over a parsed JSON value — what a template literal does to
// whatever it is handed.
//
// The reference writes messages like `Frame ${node.name} has kind ${node.kind}` over fields whose
// type says string and whose *input* can be anything a document contains, and the message it
// produces is part of the frozen output. So the rendering of a value that is not the declared type
// is not a detail: `${undefined}` is the word "undefined", `${null}` is "null", a number goes through
// Number::toString (NumberToString, never the canonical writer), an array is its elements joined
// with commas and an object is "[object Object]".
//
// present is false for a field that was absent, which is JavaScript's undefined — a different
// rendering from a field that was present and null, exactly as rule 5 says they are different facts.
func ToString(value canonical.Value, present bool) string {
	if !present {
		return "undefined"
	}
	switch value.Kind() {
	case canonical.KindNull:
		return "null"
	case canonical.KindBool:
		truth, _ := value.Bool()
		if truth {
			return "true"
		}
		return "false"
	case canonical.KindNumber:
		number, _ := value.Number()
		return NumberToString(number)
	case canonical.KindString:
		text, _ := value.Str()
		return text
	case canonical.KindArray:
		// Array.prototype.join: a null or undefined element is the empty string rather than "null",
		// which is the one place ToString's answer for null is not the word.
		parts := make([]string, 0, value.Len())
		for _, element := range value.Items() {
			if element.IsNull() {
				parts = append(parts, "")
				continue
			}
			parts = append(parts, ToString(element, true))
		}
		return strings.Join(parts, ",")
	case canonical.KindObject:
		return "[object Object]"
	}
	return "undefined"
}
