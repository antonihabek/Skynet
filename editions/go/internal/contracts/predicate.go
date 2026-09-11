package contracts

import (
	"math"
	"regexp"
	"strconv"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// Declarative predicate evaluation (§54.2).
//
// Pure and total: it never fails on a malformed rule, and a rule it cannot decide does not pass.
// Totality is not decoration — rules are operator-supplied rows in a database, and one bad row
// taking down the compatibility check for every user is the failure the reference records having
// shipped.
//
// # The two answers, and why the boolean is derived
//
// `EvaluateOutcome` returns three values — satisfied, unsatisfied, unevaluable — because §54.2 asks
// a question a boolean cannot answer: *a precondition that cannot be evaluated refuses the skill,
// naming the path that was unresolvable.* `false` refuses, which is the right direction, but it
// collapses "the world is not ready" and "this manifest names something this robot does not
// expose" into one answer, and those have different remedies. `Evaluate` is `satisfied(...)` of the
// outcome and nothing else, so the refinement cannot become a second opinion.
//
// # `undefined` is a value here, and Go has no such thing
//
// The whole three-valued design turns on telling *absent* from *null*: `readPath` returns undefined
// for a missing link, `resolved` is `value !== undefined`, and a literal `null` in a rule is
// resolved. Go's zero value cannot carry that distinction, so it is carried explicitly in `maybe`
// and in `operand.resolved`. An edition that used a nil Value for both would answer `satisfied`
// where the reference answers `unevaluable` on every one of the five inputs the reference's own
// header table lists — all five of which are in the corpus.

// maybe is a JSON value or ECMAScript's `undefined`.
type maybe struct {
	value   canonical.Value
	defined bool
}

func defined(value canonical.Value) maybe { return maybe{value: value, defined: true} }
func undefined() maybe                    { return maybe{} }

// arrayIndexPattern is the reference's `/^(0|[1-9]\d*)$/`: a canonical decimal index, which is what
// a JSON array member is named. `\d` spelt out, as everywhere in this package.
var arrayIndexPattern = regexp.MustCompile(`^(0|[1-9][0-9]*)$`)

// ReadPath reads `a.b.c` out of a context, returning undefined for any missing link.
//
// **Members only.** An array segment must be a decimal index, so `capabilities.length` does not
// resolve: `length` is a property of a JavaScript array rather than a member of the JSON document,
// and an edition without JavaScript arrays could not reproduce it. The same restriction keeps a
// path off the prototype chain, where `constructor` and `__proto__` live and where a path composed
// by somebody else has no business going. In Go there is no prototype chain to fall off, and the
// rule is implemented anyway — an edition that resolved `length` because its language made that
// easy would diverge on a path the reference refuses.
func ReadPath(context maybe, path string, hasPath bool) maybe {
	if !hasPath || path == "" {
		return undefined()
	}
	current := context
	for _, segment := range splitOn(path, '.') {
		if !current.defined || current.value.IsNull() {
			return undefined()
		}
		switch current.value.Kind() {
		case canonical.KindArray:
			if !arrayIndexPattern.MatchString(segment) {
				return undefined()
			}
			index, err := strconv.Atoi(segment)
			if err != nil {
				// An index too long for an int is not an index into any array that exists.
				return undefined()
			}
			if index >= current.value.Len() {
				return undefined()
			}
			current = defined(current.value.At(index))
		case canonical.KindObject:
			found, present := current.value.Get(segment)
			if !present {
				return undefined()
			}
			current = defined(found)
		default:
			// `typeof current !== 'object'` — a string, number or boolean has no members here,
			// which is why `"abc".length` does not resolve either.
			return undefined()
		}
	}
	return current
}

// splitOn is strings.Split for one byte, written out so the reference's `path.split('.')` has an
// obvious counterpart. An empty path yields one empty segment, exactly as JavaScript's does.
func splitOn(text string, separator byte) []string {
	parts := []string{}
	start := 0
	for at := 0; at < len(text); at++ {
		if text[at] == separator {
			parts = append(parts, text[start:at])
			start = at + 1
		}
	}
	return append(parts, text[start:])
}

// referencedPath is the path a rule value references, or absent when it is a literal.
//
//	{ "__ref": "subject.required_capabilities" }
//
// is what lets one rule express "robot capabilities must cover whatever this policy declares"
// without the rule author knowing the policy.
func referencedPath(value maybe) (string, bool) {
	// `!value` is falsy: undefined, null, false, 0, NaN and "" all take this branch.
	if !value.defined || !truthyValue(value.value) {
		return "", false
	}
	if value.value.Kind() != canonical.KindObject {
		return "", false
	}
	if ref, ok := value.value.Field("__ref").Str(); ok {
		return ref, true
	}
	// `{ path: … }` is accepted as an alias so hand-written rules read naturally — but only when it
	// is the *whole* object, which is what `Object.keys(record).length === 1` says.
	if path, ok := value.value.Field("path").Str(); ok && value.value.Len() == 1 {
		return path, true
	}
	return "", false
}

// truthyValue is JavaScript's truthiness over a parsed JSON value.
func truthyValue(value canonical.Value) bool {
	switch value.Kind() {
	case canonical.KindNull:
		return false
	case canonical.KindBool:
		flag, _ := value.Bool()
		return flag
	case canonical.KindNumber:
		number, _ := value.Number()
		return js.TruthyNumber(number)
	case canonical.KindString:
		text, _ := value.Str()
		return js.Truthy(text)
	}
	// An object or an array is truthy however empty it is.
	return true
}

// pathOf is the predicate's own path, or absent when it has none this edition can read.
func pathOf(predicate canonical.Value) (string, bool) {
	path, ok := predicate.Field("path").Str()
	if !ok || path == "" {
		return "", false
	}
	return path, true
}

// childrenOf returns the children of a composite, and whether `predicates` was readable.
//
// Absent and non-array are deliberately different answers. Absent is *no children*, and a
// conjunction over no children is satisfied — a skill declaring no preconditions does not fail
// them. A `predicates` that is a string is a **broken rule**, and answering "no children" there
// would turn it into a pass, which is the failure this module exists to stop.
func childrenOf(predicate canonical.Value) ([]canonical.Value, bool) {
	children, present := predicate.Get("predicates")
	if !present || children.IsNull() {
		return nil, true
	}
	if children.Kind() != canonical.KindArray {
		return nil, false
	}
	return children.Items(), true
}

// toArray is the reference's: an array is itself, absent and null are empty, anything else is a
// single-element list.
func toArray(value maybe) []canonical.Value {
	if !value.defined {
		return nil
	}
	if value.value.Kind() == canonical.KindArray {
		return value.value.Items()
	}
	if value.value.IsNull() {
		return nil
	}
	return []canonical.Value{value.value}
}

// toNumber is the reference's: a finite number is itself, a non-blank string goes through
// `Number()`, and everything else is not a number.
func toNumber(value maybe) (float64, bool) {
	if !value.defined {
		return 0, false
	}
	if number, ok := value.value.Number(); ok {
		if canonical.Finite(number) {
			return number, true
		}
		return 0, false
	}
	if text, ok := value.value.Str(); ok && js.Trim(text) != "" {
		parsed := js.StringToNumber(text)
		if canonical.Finite(parsed) {
			return parsed, true
		}
	}
	return 0, false
}

// ── the three-valued outcome ────────────────────────────────────────────────

// PredicateOutcome is what an evaluation decided, and why it could not.
type PredicateOutcome struct {
	Outcome string
	// Reason is why, when unevaluable. Absent otherwise.
	Reason    string
	HasReason bool
	// Path is the path that could not be resolved or read, when there is one.
	Path    string
	HasPath bool
	// Op is the operator that could not decide. Absent when there was no readable operator at all.
	Op    string
	HasOp bool
}

func decidedOutcome(value bool) PredicateOutcome {
	if value {
		return PredicateOutcome{Outcome: "satisfied"}
	}
	return PredicateOutcome{Outcome: "unsatisfied"}
}

func unevaluable(reason string, path string, hasPath bool, op string, hasOp bool) PredicateOutcome {
	return PredicateOutcome{
		Outcome: "unevaluable",
		Reason:  reason, HasReason: true,
		Path: path, HasPath: hasPath,
		Op: op, HasOp: hasOp,
	}
}

// Satisfied reports whether an outcome permits whatever it was guarding. Only `satisfied` does.
func Satisfied(outcome PredicateOutcome) bool { return outcome.Outcome == "satisfied" }

// operand is one side of a comparison.
//
// `path` is where the value came from, or absent for a literal, and it is load-bearing twice: it is
// what a refusal names, and it is how `identical` decides object equality without a reference to
// compare.
type operand struct {
	value   maybe
	path    string
	hasPath bool
	// resolved is false when the path named nothing. A literal is always resolved, even a literal
	// null.
	resolved bool
}

func fromPath(context maybe, path string) operand {
	value := ReadPath(context, path, true)
	return operand{value: value, path: path, hasPath: true, resolved: value.defined}
}

func fromValue(value maybe, context maybe) operand {
	ref, isRef := referencedPath(value)
	if !isRef {
		return operand{value: value, resolved: true}
	}
	return fromPath(context, ref)
}

// element is a member of a collection operand, which is a member of the context in its own right.
func element(collection operand, item canonical.Value, index int) operand {
	found := operand{value: defined(item), resolved: true}
	if collection.hasPath {
		found.path, found.hasPath = collection.path+"."+strconv.Itoa(index), true
	}
	return found
}

// firstUnresolved is the first operand that named nothing, as a refusal.
func firstUnresolved(op string, operands ...operand) (PredicateOutcome, bool) {
	for _, item := range operands {
		if !item.resolved {
			return unevaluable("unresolved_path", item.path, item.hasPath, op, true), true
		}
	}
	return PredicateOutcome{}, false
}

// identical is the specification's equality, written here rather than left to the host language.
//
// JavaScript's `===` on two objects is reference equality, and the reference *is* the
// specification, so the rule other editions must reproduce is stated:
//
//   - both primitives — undefined, null, a boolean, a number or a string — and equal; or
//   - both read from the **same path of the same context**, which is the only way two non-primitive
//     operands can be one object.
//
// A literal object or array in a rule is therefore never equal to anything, which is exactly what
// `===` does and is worth knowing before writing `{"op":"eq","value":["a"]}` and wondering why it
// never fires. `includes` is the operator for that.
func identical(left, right operand) bool {
	if isPrimitive(left.value) && isPrimitive(right.value) {
		return strictEquals(left.value, right.value)
	}
	if !left.hasPath || !right.hasPath {
		return false
	}
	return left.path == right.path
}

func isPrimitive(value maybe) bool {
	if !value.defined {
		return true // `undefined` is a primitive
	}
	switch value.value.Kind() {
	case canonical.KindArray, canonical.KindObject:
		return false
	}
	return true
}

// strictEquals is `===` over two primitives.
//
// Two properties of it are not Go's `==` and both are reachable: NaN is equal to nothing including
// itself, and +0 equals -0. A struct comparison would get the first wrong and the second right,
// which is the worst of the two.
func strictEquals(left, right maybe) bool {
	if left.defined != right.defined {
		return false
	}
	if !left.defined {
		return true // undefined === undefined
	}
	if left.value.Kind() != right.value.Kind() {
		return false
	}
	switch left.value.Kind() {
	case canonical.KindNull:
		return true
	case canonical.KindBool:
		leftFlag, _ := left.value.Bool()
		rightFlag, _ := right.value.Bool()
		return leftFlag == rightFlag
	case canonical.KindNumber:
		leftNumber, _ := left.value.Number()
		rightNumber, _ := right.value.Number()
		if math.IsNaN(leftNumber) || math.IsNaN(rightNumber) {
			return false
		}
		return leftNumber == rightNumber
	case canonical.KindString:
		leftText, _ := left.value.Str()
		rightText, _ := right.value.Str()
		return leftText == rightText
	}
	return false
}

// contains is membership, with identical's rule applied element by element.
func contains(collection operand, needle operand) bool {
	for index, item := range toArray(collection.value) {
		if identical(element(collection, item, index), needle) {
			return true
		}
	}
	return false
}

// EvaluateOutcome is the evaluator. Everything else here is a view of it.
func EvaluateOutcome(predicate maybe, context maybe) PredicateOutcome {
	if !predicate.defined || predicate.value.Kind() != canonical.KindObject {
		return unevaluable("malformed", "", false, "", false)
	}
	op, ok := predicate.value.Field("op").Str()
	if !ok {
		return unevaluable("malformed", "", false, "", false)
	}
	node := predicate.value

	switch op {
	// Order is part of the answer for all three: a refusal names *a* predicate, and which one it
	// names must not depend on how an edition iterates.
	case "and":
		children, readable := childrenOf(node)
		if !readable {
			return unevaluable("malformed", "", false, op, true)
		}
		for _, child := range children {
			outcome := EvaluateOutcome(defined(child), context)
			if outcome.Outcome != "satisfied" {
				return outcome
			}
		}
		// Nothing was asked for. A skill declaring no preconditions is not one that fails them.
		return decidedOutcome(true)

	case "or":
		children, readable := childrenOf(node)
		if !readable {
			return unevaluable("malformed", "", false, op, true)
		}
		var firstUnevaluable PredicateOutcome
		haveUnevaluable := false
		for _, child := range children {
			outcome := EvaluateOutcome(defined(child), context)
			if outcome.Outcome == "satisfied" {
				return outcome
			}
			if outcome.Outcome == "unevaluable" && !haveUnevaluable {
				firstUnevaluable, haveUnevaluable = outcome, true
			}
		}
		// An alternative that held would have returned already; an alternative nobody could read
		// means this disjunction is not known to be false, only unread.
		if haveUnevaluable {
			return firstUnevaluable
		}
		return decidedOutcome(false)

	case "not":
		children, readable := childrenOf(node)
		if !readable {
			return unevaluable("malformed", "", false, op, true)
		}
		// `predicate.predicate ?? children[0]` — nullish, so a `predicate` that is present and null
		// falls through to the first child rather than being negated.
		child := undefined()
		if own, present := node.Get("predicate"); present && !own.IsNull() {
			child = defined(own)
		} else if len(children) > 0 {
			child = defined(children[0])
		}
		if !child.defined {
			return unevaluable("missing_operand", "", false, op, true)
		}
		outcome := EvaluateOutcome(child, context)
		// Ignorance does not negate into knowledge, and this is the line that used to say it did.
		if outcome.Outcome == "unevaluable" {
			return outcome
		}
		return decidedOutcome(outcome.Outcome == "unsatisfied")

	case "exists":
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		// The one operator that answers *about* resolution, so it never refuses for lack of it.
		value := ReadPath(context, path, true)
		if !value.defined || value.value.IsNull() {
			return decidedOutcome(false)
		}
		if value.value.Kind() == canonical.KindArray {
			return decidedOutcome(value.value.Len() > 0)
		}
		if text, isText := value.value.Str(); isText {
			return decidedOutcome(js.Trim(text) != "")
		}
		return decidedOutcome(true)

	case "eq", "neq":
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		left := fromPath(context, path)
		right := fromValue(field(node, "value"), context)
		if refused, found := firstUnresolved(op, left, right); found {
			return refused
		}
		equal := identical(left, right)
		if op == "eq" {
			return decidedOutcome(equal)
		}
		return decidedOutcome(!equal)

	case "in":
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		needle := fromPath(context, path)
		collection := fromValue(field(node, "value"), context)
		if refused, found := firstUnresolved(op, needle, collection); found {
			return refused
		}
		return decidedOutcome(contains(collection, needle))

	case "includes":
		// The context value is a collection that must contain the rule's value.
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		collection := fromPath(context, path)
		needle := fromValue(field(node, "value"), context)
		if refused, found := firstUnresolved(op, collection, needle); found {
			return refused
		}
		return decidedOutcome(contains(collection, needle))

	case "includes_all", "includes_any":
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		collection := fromPath(context, path)
		wanted := fromValue(field(node, "value"), context)
		if refused, found := firstUnresolved(op, collection, wanted); found {
			return refused
		}
		items := toArray(wanted.value)
		any, all := false, true
		for index, item := range items {
			if contains(collection, element(wanted, item, index)) {
				any = true
			} else {
				all = false
			}
		}
		// An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from
		// a requirement that *is* empty, never from one nobody could read.
		if op == "includes_any" {
			return decidedOutcome(len(items) == 0 || any)
		}
		return decidedOutcome(all)

	case "gte", "lte", "gt", "lt":
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		leftOperand := fromPath(context, path)
		rightOperand := fromValue(field(node, "value"), context)
		if refused, found := firstUnresolved(op, leftOperand, rightOperand); found {
			return refused
		}
		left, okLeft := toNumber(leftOperand.value)
		if !okLeft {
			return unevaluable("not_a_number", leftOperand.path, leftOperand.hasPath, op, true)
		}
		right, okRight := toNumber(rightOperand.value)
		if !okRight {
			return unevaluable("not_a_number", rightOperand.path, rightOperand.hasPath, op, true)
		}
		switch op {
		case "gte":
			return decidedOutcome(left >= right)
		case "lte":
			return decidedOutcome(left <= right)
		case "gt":
			return decidedOutcome(left > right)
		}
		return decidedOutcome(left < right)

	case "semver_satisfies":
		path, hasPath := pathOf(node)
		if !hasPath {
			return unevaluable("missing_path", "", false, op, true)
		}
		versionOperand := fromPath(context, path)
		rangeOperand := fromValue(field(node, "value"), context)
		if refused, found := firstUnresolved(op, versionOperand, rangeOperand); found {
			return refused
		}
		// A version this edition cannot parse is a fact about the declaration, not a version that
		// fell outside the window — so it is named rather than counted as a miss.
		version, isVersionText := stringOf(versionOperand.value)
		if !isVersionText || !IsValidVersion(version) {
			return unevaluable("not_a_version", versionOperand.path, versionOperand.hasPath, op, true)
		}
		rangeText, isRangeText := stringOf(rangeOperand.value)
		if !isRangeText || !IsValidRange(rangeText) {
			return unevaluable("not_a_version", rangeOperand.path, rangeOperand.hasPath, op, true)
		}
		return decidedOutcome(Satisfies(version, rangeText))
	}

	path, hasPath := pathOf(node)
	return unevaluable("unknown_op", path, hasPath, op, true)
}

// field reads a member as a maybe, so an absent one is `undefined` rather than null.
func field(object canonical.Value, key string) maybe {
	value, present := object.Get(key)
	if !present {
		return undefined()
	}
	return defined(value)
}

func stringOf(value maybe) (string, bool) {
	if !value.defined {
		return "", false
	}
	return value.value.Str()
}

// EvaluatePredicate is the compatibility engine's answer: does this rule hold?
//
// Derived, not reimplemented. Two functions computing the same thing by two routes is the drift
// this repository exists to prevent.
func EvaluatePredicate(predicate maybe, context maybe) bool {
	return Satisfied(EvaluateOutcome(predicate, context))
}

// ── the corpus ──────────────────────────────────────────────────────────────

func predicateEvaluate(input canonical.Value) (canonical.Value, error) {
	predicate := field(input, "predicate")
	context := field(input, "context")
	outcome := EvaluateOutcome(predicate, context)
	return canonical.Object().
		Set("satisfied", canonical.Bool(Satisfied(outcome))).
		Set("outcome", canonical.String(outcome.Outcome)).
		Set("reason", optionalString(outcome.Reason, outcome.HasReason)).
		Set("path", optionalString(outcome.Path, outcome.HasPath)).
		Set("op", optionalString(outcome.Op, outcome.HasOp)), nil
}
