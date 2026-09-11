package canonical

import (
	"errors"
	"math"
	"sort"
	"strconv"
	"strings"
)

// intSafe is 2^53, the boundary rule 3 draws between "write this as an integer" and "write the
// shortest decimal that round-trips". Above it, consecutive integers are no longer all
// representable, so `String(value)` in the reference stops being the identity it is below.
const intSafe = 9007199254740992.0

// ErrNonFinite is returned when a value that is not finite reaches the writer.
//
// Loud rather than lenient, and the loudness is the feature: a NaN emitted as `null` would turn
// "this was not measured" into "this was measured as nothing", which is a distinction the whole
// system is built on — corpus/README.md spends a section on it and §74.3 a whole rule. An edition
// that quietly wrote `null` here would agree with the corpus on every case that has no NaN in it
// and be wrong about the one fact those cases exist to carry.
var ErrNonFinite = errors.New("canonical: cannot serialise a non-finite number")

// FormatNumber renders a number in canonical form.
//
// The shape of this function is the reference's, deliberately: try one significant digit, then
// two, and stop at the first rendering that parses back to exactly the value given. That is the
// definition of "the shortest decimal that round-trips", and writing it as a search rather than
// reaching for the platform's shortest-form printer is what keeps the last digit the reference's
// in the cases where two shortest forms both round-trip. Go's own shortest form (a -1 precision)
// agrees on every input the corpus holds and is not *defined* to break a tie the way ECMAScript's
// toExponential is, so the search is what is written.
func FormatNumber(value float64) (string, error) {
	if !Finite(value) {
		return "", ErrNonFinite
	}
	// Covers -0, which rule 3 says is written `0`. Go's == does that for us: -0 == 0 is true, and
	// it is the one comparison in this package where that is the answer wanted rather than a bug.
	if value == 0 {
		return "0", nil
	}
	if value == math.Trunc(value) && math.Abs(value) < intSafe {
		return strconv.FormatFloat(value, 'f', -1, 64), nil
	}
	for precision := 1; precision <= 17; precision++ {
		rendered := strconv.FormatFloat(value, 'e', precision-1, 64)
		if back, err := strconv.ParseFloat(rendered, 64); err == nil && back == value {
			return exponentialToPositional(rendered), nil
		}
	}
	// Unreachable: 17 significant digits round-trip every float64. Written rather than assumed,
	// because the alternative to a fallback here is returning "" and a nil error.
	return exponentialToPositional(strconv.FormatFloat(value, 'e', 16, 64)), nil
}

// exponentialToPositional turns `-4.4e-01` into `-0.44`.
//
// Rule 3's "always positional, never exponential" is the clause that makes this whole function
// necessary — every language's default float printer switches to exponential somewhere, and no two
// of them switch at the same place. Writing the digits out means the corpus never has to care.
func exponentialToPositional(rendered string) string {
	marker := strings.IndexByte(rendered, 'e')
	if marker < 0 {
		return rendered
	}
	mantissa := rendered[:marker]
	// Go writes the exponent with an explicit sign and at least two digits (`e+07`); ECMAScript
	// writes `e+7`. Atoi reads both, which is why the exponent is parsed rather than sliced.
	exponent, err := strconv.Atoi(rendered[marker+1:])
	if err != nil {
		return rendered
	}

	negative := strings.HasPrefix(mantissa, "-")
	if negative {
		mantissa = mantissa[1:]
	}
	digits := strings.Replace(mantissa, ".", "", 1)

	// The first digit carries weight 10^exponent, so the point falls after exponent+1 digits.
	point := exponent + 1

	var out strings.Builder
	if negative {
		out.WriteByte('-')
	}
	switch {
	case point <= 0:
		out.WriteString("0.")
		out.WriteString(strings.Repeat("0", -point))
		out.WriteString(digits)
	case point >= len(digits):
		out.WriteString(digits)
		out.WriteString(strings.Repeat("0", point-len(digits)))
	default:
		out.WriteString(digits[:point])
		out.WriteByte('.')
		out.WriteString(digits[point:])
	}
	return out.String()
}

// writeString applies rule 4.
//
// Iterating bytes rather than runes is not a shortcut: every character rule 4 escapes is below
// 0x80, and UTF-8 encodes a byte below 0x80 only as itself — a continuation byte is always ≥ 0x80.
// So a byte scan sees exactly the characters a code-point scan would act on, passes every other
// byte through untouched, and never has to decode. It also means text that is not valid UTF-8
// survives unchanged rather than being replaced with U+FFFD, which matters because the corpus
// compares bytes and a silent repair is a difference no reader would think to look for.
func writeString(value string, out *strings.Builder) {
	out.WriteByte('"')
	for at := 0; at < len(value); at++ {
		ch := value[at]
		switch ch {
		case '"':
			out.WriteString("\\\"")
		case '\\':
			out.WriteString("\\\\")
		case '\b':
			out.WriteString("\\b")
		case '\f':
			out.WriteString("\\f")
		case '\n':
			out.WriteString("\\n")
		case '\r':
			out.WriteString("\\r")
		case '\t':
			out.WriteString("\\t")
		default:
			if ch < 0x20 {
				out.WriteString("\\u")
				const hex = "0123456789abcdef"
				out.WriteByte('0')
				out.WriteByte('0')
				out.WriteByte(hex[ch>>4])
				out.WriteByte(hex[ch&0xf])
			} else {
				out.WriteByte(ch)
			}
		}
	}
	out.WriteByte('"')
}

func writeValue(value Value, out *strings.Builder) error {
	switch value.kind {
	case KindNull:
		out.WriteString("null")
		return nil
	case KindBool:
		if value.boolean {
			out.WriteString("true")
		} else {
			out.WriteString("false")
		}
		return nil
	case KindNumber:
		rendered, err := FormatNumber(value.number)
		if err != nil {
			return err
		}
		out.WriteString(rendered)
		return nil
	case KindString:
		writeString(value.text, out)
		return nil
	case KindArray:
		out.WriteByte('[')
		for at, item := range value.array {
			if at > 0 {
				out.WriteByte(',')
			}
			if err := writeValue(item, out); err != nil {
				return err
			}
		}
		out.WriteByte(']')
		return nil
	case KindObject:
		// Rule 2 is applied here and not in the container: Members keeps insertion order because
		// the contracts read it, and the sort is a property of the *encoding*. Go's string
		// comparison is a byte comparison, which is exactly the UTF-8 byte ordering rule 2 asks
		// for — it differs from code-point ordering above the BMP, and an edition that sorted by
		// code point would agree for ASCII and diverge on the first vendor channel name that was
		// not ASCII.
		order := make([]int, len(value.object))
		for at := range order {
			order[at] = at
		}
		sort.SliceStable(order, func(left, right int) bool {
			return value.object[order[left]].Key < value.object[order[right]].Key
		})
		out.WriteByte('{')
		for at, index := range order {
			if at > 0 {
				out.WriteByte(',')
			}
			writeString(value.object[index].Key, out)
			out.WriteByte(':')
			if err := writeValue(value.object[index].Value, out); err != nil {
				return err
			}
		}
		out.WriteByte('}')
		return nil
	}
	return errors.New("canonical: value of unknown kind")
}

// Encode serialises a value in canonical form.
func Encode(value Value) (string, error) {
	var out strings.Builder
	if err := writeValue(value, &out); err != nil {
		return "", err
	}
	return out.String(), nil
}
