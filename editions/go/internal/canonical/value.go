// Canonical JSON, the fifth implementation of one rule.
//
// `crates/skynet-json` is the first, `reference/typescript/canonical.ts` the second,
// `editions/python/skynet_os/canonical.py` the third and `editions/cpp/src/json.cpp` the fourth.
// The duplication is the point: §31.4 requires every edition to reproduce the corpus
// **byte-identically**, and a corpus whose expected values were produced by the same code that
// checks them would prove nothing.
//
// The rule, restated so this file can be read on its own:
//
//  1. No insignificant whitespace.
//  2. Object keys sorted by the byte sequence of their UTF-8 encoding.
//  3. Numbers are finite. An integral value below 2^53 is written as an integer, `-0` as `0`;
//     anything else as the shortest decimal that round-trips, always positional, never
//     exponential.
//  4. Strings escape `"`, `\` and the C0 controls and nothing else.
//  5. Absent fields are omitted; `null` is written as `null`. The two are different facts — the
//     whole system distinguishes "nobody declared a safety envelope" from "an envelope was
//     declared and is empty".
//
// # Why this is not map[string]any
//
// Two independent reasons, and each one alone would be enough.
//
// **Iteration order.** Go randomises map iteration deliberately, and the reference edition
// iterates objects in *insertion* order — several contracts' output depends on it. `evaluateCycle`
// walks the commanded channels and appends one entry to `clamped` per channel it reduced, so that
// array's order is the order the channels appeared in the document. A Go edition built on a map
// would emit the same set of clamps in a different order *on every run*, which is the "a warning
// in a different order today is a different clamp tomorrow" case §31.4 spends a rule on.
// `editions/cpp/include/skynet_os/json.hpp` records this costing a rewrite; it is recorded here
// because Go's randomisation makes it a flake rather than a stable wrong answer, which is worse.
//
// **Absent is not null.** map[string]any cannot hold rule 5's distinction without a sentinel, and a
// sentinel is a value that can leak into output. Here a field that is absent is a Set that was
// never called, so there is no state in which a stale one survives — rule 5 is unrepresentable
// rather than enforced, which is the stronger of the two (§6.1's first gate).
package canonical

import "math"

// Kind is the JSON type of a Value. Six kinds, closed, checked exhaustively — §3.2's "closed union
// or enum type" rather than a free-form string compared with ==.
type Kind uint8

const (
	// KindNull is `null`, which rule 5 distinguishes from a field that is absent.
	KindNull Kind = iota
	KindBool
	KindNumber
	KindString
	KindArray
	KindObject
)

// String names the kind, for the error messages a dispatch produces when an input is not the shape
// its contract expects.
func (k Kind) String() string {
	switch k {
	case KindNull:
		return "null"
	case KindBool:
		return "boolean"
	case KindNumber:
		return "number"
	case KindString:
		return "string"
	case KindArray:
		return "array"
	case KindObject:
		return "object"
	}
	return "unknown"
}

// Member is one key/value pair of an object, held in insertion order.
type Member struct {
	Key   string
	Value Value
}

// Value is a JSON value.
//
// The fields are unexported and every one is reached through a constructor or an accessor, so a
// Value whose kind says string and whose number field holds 3 cannot be built (R-01). The zero
// Value is `null`, which is the one default that cannot be mistaken for data.
type Value struct {
	kind    Kind
	boolean bool
	number  float64
	text    string
	array   []Value
	object  []Member
}

// Null returns the JSON `null`. Distinct from an absent field: see rule 5.
func Null() Value { return Value{kind: KindNull} }

// Bool returns a JSON boolean.
func Bool(value bool) Value { return Value{kind: KindBool, boolean: value} }

// Number returns a JSON number.
//
// Non-finite values are accepted here and rejected by the writer rather than at construction,
// because NaN is a value the contracts legitimately compute with — corpus/README.md's rule that a
// `null` input against a bare `number` type reads as NaN puts one into the very first step of
// several dispatches. What must never happen is a NaN reaching the *wire*, and that is the
// writer's job, where it is one check rather than a hundred.
func Number(value float64) Value { return Value{kind: KindNumber, number: value} }

// String returns a JSON string.
func String(value string) Value { return Value{kind: KindString, text: value} }

// Array returns a JSON array holding the given values.
func Array(values ...Value) Value {
	// A non-nil empty slice, so a built-empty array and a parsed-empty one are the same value.
	items := make([]Value, 0, len(values))
	items = append(items, values...)
	return Value{kind: KindArray, array: items}
}

// Object returns an empty JSON object. Fields are added with Set, in the order the reference adds
// them; the writer sorts.
func Object() Value {
	return Value{kind: KindObject, object: make([]Member, 0, 8)}
}

// Kind reports the JSON type of the value.
func (v Value) Kind() Kind { return v.kind }

// IsNull reports whether the value is `null`.
func (v Value) IsNull() bool { return v.kind == KindNull }

// Bool returns the boolean and whether the value is one.
func (v Value) Bool() (bool, bool) { return v.boolean, v.kind == KindBool }

// Number returns the number and whether the value is one.
func (v Value) Number() (float64, bool) { return v.number, v.kind == KindNumber }

// Str returns the string and whether the value is one.
//
// Named Str rather than String because a `String() string` method would make Value a fmt.Stringer,
// and then every %v in the edition would print a value through a path that is not the canonical
// writer — two renderings of one value, which is the thing this package exists to prevent.
func (v Value) Str() (string, bool) { return v.text, v.kind == KindString }

// Len returns the number of elements in an array or fields in an object, and 0 for anything else.
func (v Value) Len() int {
	switch v.kind {
	case KindArray:
		return len(v.array)
	case KindObject:
		return len(v.object)
	}
	return 0
}

// At returns the element at index of an array. Out of range, or not an array, returns `null` —
// which is what a reader of a shorter array than it expected should see.
func (v Value) At(index int) Value {
	if v.kind != KindArray || index < 0 || index >= len(v.array) {
		return Null()
	}
	return v.array[index]
}

// Items returns the elements of an array, or nil.
func (v Value) Items() []Value {
	if v.kind != KindArray {
		return nil
	}
	return v.array
}

// Members returns an object's fields in insertion order, or nil.
func (v Value) Members() []Member {
	if v.kind != KindObject {
		return nil
	}
	return v.object
}

// Get returns the named field and whether it is present.
//
// Present-with-`null` and absent are different answers here, exactly as they are on the wire: the
// bool distinguishes them, and a caller that ignores it has written the bug rule 5 is about.
func (v Value) Get(key string) (Value, bool) {
	if v.kind != KindObject {
		return Null(), false
	}
	for _, member := range v.object {
		if member.Key == key {
			return member.Value, true
		}
	}
	return Null(), false
}

// Field returns the named field, or `null` when it is absent.
//
// For the many readers to which absent and null mean the same thing — a missing optional whose
// default is "nothing declared". A reader that must tell them apart calls Get.
func (v Value) Field(key string) Value {
	found, _ := v.Get(key)
	return found
}

// Has reports whether the named field is present, including when its value is `null`.
func (v Value) Has(key string) bool {
	_, present := v.Get(key)
	return present
}

// Set adds or replaces a field, and returns the receiver so building reads in one expression.
//
// Replacing keeps the field's original position, because the position is insertion order and the
// reference's `object[key] = x` on an existing key does not move it either.
func (v Value) Set(key string, value Value) Value {
	if v.kind != KindObject {
		panic("canonical: Set on a " + v.kind.String())
	}
	for at := range v.object {
		if v.object[at].Key == key {
			v.object[at].Value = value
			return v
		}
	}
	v.object = append(v.object, Member{Key: key, Value: value})
	return v
}

// SetIf adds the field only when present is true, and is how rule 5's "absent" is written.
//
// The alternative — setting a sentinel and filtering at the writer — is what the Python edition
// needs because a dict cannot hold "this key is not here". Go can simply not call Set, and this
// helper exists so the reference's `...(x !== undefined ? {k: x} : {})` translates to one line
// rather than to a three-line `if` whose body is easy to get backwards.
func (v Value) SetIf(key string, value Value, present bool) Value {
	if !present {
		return v
	}
	return v.Set(key, value)
}

// Push appends to an array and returns the receiver.
func (v Value) Push(value Value) Value {
	if v.kind != KindArray {
		panic("canonical: Push onto a " + v.kind.String())
	}
	v.array = append(v.array, value)
	return v
}

// Numbers returns an array of numbers, which is the shape most of the kinematics output takes.
func Numbers(values []float64) Value {
	out := Array()
	for _, value := range values {
		out = out.Push(Number(value))
	}
	return out
}

// Strings returns an array of strings.
func Strings(values []string) Value {
	out := Array()
	for _, value := range values {
		out = out.Push(String(value))
	}
	return out
}

// Finite reports whether a number may be written. Exported because the contracts ask the same
// question about their inputs, and asking it two ways is how two answers happen.
func Finite(value float64) bool {
	return !math.IsNaN(value) && !math.IsInf(value, 0)
}
