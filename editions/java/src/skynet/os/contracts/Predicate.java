// Declarative predicate evaluation — `reference/typescript/predicate.ts`.
//
// Compatibility rules are data, not code, so a third party can register a robot or a hardware constraint
// without shipping a frontend change. The module is pure and **total**: it never fails on a malformed rule, and
// a rule it cannot decide does not pass.
//
// # The two answers, and why the boolean is derived
//
// §54.2 asks a question a boolean cannot answer: *a precondition that cannot be evaluated refuses the skill,
// naming the path that was unresolvable.* `false` refuses, which is the right direction, but it collapses "the
// world is not ready" and "this manifest names something this robot does not expose" into one answer — a
// battery that needs charging and a skill that will never run here, whose publisher needs telling.
//
// So there is one evaluator, and the boolean is `outcome == SATISFIED` of it. Two functions computing the same
// thing by two routes is the drift this repository exists to prevent, and a refinement that cannot disagree
// with the thing it refines has nothing to drift from.
//
// # Where ignorance used to pass
//
// Five inputs answered `true` before the outcome existed, all for the same reason — JavaScript's `undefined`
// arriving at `!`, `!==` and an `every` over nothing — and all now refuse. The one that mattered: an
// `includes_all` of the robot's capabilities against a policy's required list **passed when that list was
// missing.** A rule that passes because the fact is absent is worse than one that fails, because nobody goes
// looking for it.

package skynet.os.contracts;

import java.util.Locale;
import skynet.os.Js;
import skynet.os.Value;

public final class Predicate {
    private Predicate() {}

    public enum Kind {
        SATISFIED, UNSATISFIED, UNEVALUABLE;

        public String label() {
            return name().toLowerCase(Locale.ROOT);
        }
    }

    /**
     * Why a predicate could not be decided.
     *
     * <p>A closed vocabulary, because §54.2 requires the refusal to <i>name</i> something and a free-text reason
     * is the same nothing as a free-text precondition. {@code corpus/vocabulary/closed-sets.json} holds every
     * edition to this list.
     */
    public enum Reason {
        /** Not an object, or the operator is not a string. There is no predicate here. */
        MALFORMED,
        /** An operator this edition does not implement. */
        UNKNOWN_OP,
        /** The predicate names no path to read. */
        MISSING_PATH,
        /** A negation with nothing to negate. */
        MISSING_OPERAND,
        /** A path that names nothing in this context. This is the one §54.2 is about. */
        UNRESOLVED_PATH,
        /** An operand is present but is not a finite number, and the operator compares numbers. */
        NOT_A_NUMBER,
        /** An operand is present but is not a version, or not a range. */
        NOT_A_VERSION;

        public String label() {
            return name().toLowerCase(Locale.ROOT);
        }
    }

    public record Outcome(Kind outcome, Reason reason, String path, String op) {}

    private static Outcome decided(boolean truth) {
        return new Outcome(truth ? Kind.SATISFIED : Kind.UNSATISFIED, null, null, null);
    }

    private static Outcome unevaluable(Reason reason, String path, String op) {
        return new Outcome(Kind.UNEVALUABLE, reason, path, op);
    }

    /**
     * A canonical decimal index, which is what a JSON array member is named: zero, or a non-zero digit followed
     * by any digits. So {@code 01}, {@code 1.0} and {@code -1} are not members of anything, and -1 is returned
     * for a segment that is not an index.
     */
    private static int canonicalIndex(String segment) {
        if (segment.isEmpty()) return -1;
        if (segment.charAt(0) == '0') return segment.length() == 1 ? 0 : -1;
        if (segment.charAt(0) < '1' || segment.charAt(0) > '9') return -1;
        long out = 0;
        for (int at = 0; at < segment.length(); at++) {
            char ch = segment.charAt(at);
            if (ch < '0' || ch > '9') return -1;
            out = out * 10 + (ch - '0');
            // A path naming an index beyond any array this document could hold resolves to nothing, which is
            // the same answer as an index past the end.
            if (out > Integer.MAX_VALUE) return -1;
        }
        return (int) out;
    }

    /**
     * Read a dotted path out of a context. Null — the reference's {@code undefined} — for any missing link.
     *
     * <p><b>Members only.</b> An array segment must be a canonical decimal index, so a path ending in
     * {@code length} does not resolve: that is a property of a JavaScript array rather than a member of the JSON
     * document, and an edition without JavaScript arrays could not reproduce it. The same restriction keeps a
     * path off the prototype chain, where a path composed by somebody else has no business going.
     */
    public static Value readPath(Value context, String path) {
        if (path == null || path.isEmpty()) return null;
        Value current = context;
        int at = 0;
        while (true) {
            int start = at;
            while (at < path.length() && path.charAt(at) != '.') at += 1;
            String segment = path.substring(start, at);

            // A null or absent link, and a link that is not an object or an array, both answer undefined: a path
            // cannot descend into a string, a number or a boolean.
            if (current == null) return null;
            if (current.kind() == Value.Kind.ARRAY) {
                int index = canonicalIndex(segment);
                if (index < 0) return null;
                // Past the end is undefined, which is null here — not a null member.
                current = Value.at(current, index);
            } else if (current.kind() == Value.Kind.OBJECT) {
                // An own-property test, so a member present and null resolves to null and a member that is
                // absent resolves to undefined. The two are different facts throughout.
                if (!Value.has(current, segment)) return null;
                current = Value.get(current, segment);
            } else {
                return null;
            }

            if (at >= path.length()) return current;
            at += 1;
        }
    }

    /**
     * One side of a comparison.
     *
     * <p>{@code path} is where the value came from, or null for a literal, and it is load-bearing twice: it is
     * what a refusal names, and it is how {@code identical} decides object equality without a reference to
     * compare. {@code resolved} is false when the path named nothing; a literal is always resolved, even a
     * literal null.
     */
    private record Operand(Value held, String path, boolean resolved) {}

    /** The path a rule value references, or null when it is a literal. */
    private static String referencedPath(Value held) {
        if (held == null || held.kind() != Value.Kind.OBJECT) return null;
        Value ref = Value.get(held, "__ref");
        if (ref != null && ref.kind() == Value.Kind.STRING) return Value.asString(ref, "");
        // A bare `path` object is accepted as an alias so hand-written rules read naturally — and only when it
        // is the *whole* object, so a predicate carrying both a path and an operator is not one.
        Value alias = Value.get(held, "path");
        if (alias != null && alias.kind() == Value.Kind.STRING && Value.length(held) == 1) {
            return Value.asString(alias, "");
        }
        return null;
    }

    private static Operand fromPath(Value context, String path) {
        Value held = readPath(context, path);
        // Resolved means "not undefined" — a path that resolved to an explicit null *is* resolved.
        return new Operand(held, path, held != null);
    }

    private static Operand fromValue(Value held, Value context) {
        String ref = referencedPath(held);
        if (ref == null) return new Operand(held, null, true);
        return fromPath(context, ref);
    }

    /**
     * An element of a collection operand, which is a member of the context in its own right — and so has a path
     * of its own, which is what makes two elements of one array distinguishable to {@code identical}.
     */
    private static Operand element(Operand collection, Value item, int index) {
        return new Operand(item, collection.path() == null ? null : collection.path() + "." + index, true);
    }

    private static boolean primitive(Value held) {
        return held == null || (held.kind() != Value.Kind.ARRAY && held.kind() != Value.Kind.OBJECT);
    }

    /**
     * Strict equality over two primitives, including the two absences: undefined equals undefined, null equals
     * null, and the two are not each other. NaN equals nothing, itself included.
     */
    private static boolean strictEqual(Value left, Value right) {
        if (left == null || right == null) return left == null && right == null;
        if (left.kind() != right.kind()) return false;
        return switch (left.kind()) {
            case NULL -> true;
            case BOOL -> Value.asBool(left, false) == Value.asBool(right, true);
            // A primitive comparison rather than {@code Double.equals}, deliberately: NaN must not equal itself
            // and negative zero must equal zero, and {@code Double.equals} answers the opposite on both.
            case NUMBER -> Value.asNumber(left, 0) == Value.asNumber(right, 1);
            case STRING -> Value.asString(left, "").equals(Value.asString(right, " "));
            case ARRAY, OBJECT -> false;
        };
    }

    /**
     * Strict equality on two objects is reference equality in JavaScript, and the reference is the
     * specification, so the rule other editions must reproduce is written here rather than left to the host
     * language.
     *
     * <p>Two operands are identical when both are primitives and equal, or when both were read from the
     * <b>same path of the same context</b>, which is the only way two non-primitive operands can be one object.
     * A literal object or array in a rule is therefore never equal to anything — which is exactly what strict
     * equality does, and is worth knowing before writing a rule whose value is a one-element array and
     * wondering why it never fires. Membership is the operator for that.
     */
    private static boolean identical(Operand left, Operand right) {
        if (primitive(left.held()) && primitive(right.held())) return strictEqual(left.held(), right.held());
        if (left.path() == null || right.path() == null) return false;
        return left.path().equals(right.path());
    }

    /** An array is itself, an absence is empty, and anything else is a list of one. */
    private static int operandLength(Value held) {
        if (held == null) return 0;
        if (held.kind() == Value.Kind.ARRAY) return Value.length(held);
        if (held.kind() == Value.Kind.NULL) return 0;
        return 1;
    }

    private static Value operandAt(Value held, int index) {
        if (held == null) return null;
        if (held.kind() == Value.Kind.ARRAY) return Value.at(held, index);
        return held;
    }

    /** Membership, with {@code identical}'s rule applied element by element. */
    private static boolean contains(Operand collection, Operand needle) {
        for (int at = 0; at < operandLength(collection.held()); at++) {
            if (identical(element(collection, operandAt(collection.held(), at), at), needle)) return true;
        }
        return false;
    }

    /**
     * A finite number is itself, a non-blank string is converted, and everything else — a boolean, an array, an
     * absence, a blank string — is not a number. Null when it is not.
     */
    private static Double toNumber(Value held) {
        if (held == null) return null;
        if (held.kind() == Value.Kind.NUMBER) {
            double number = Value.asNumber(held, 0);
            return Double.isFinite(number) ? number : null;
        }
        if (held.kind() == Value.Kind.STRING) {
            String text = Value.asString(held, "");
            // A blank string converts to zero, and the reference refuses it before converting: a robot that
            // reported three spaces for its battery has not reported a number.
            if (Js.trim(text).isEmpty()) return null;
            double parsed = Js.stringToNumber(text);
            return Double.isFinite(parsed) ? parsed : null;
        }
        return null;
    }

    /** The predicate's own path, or null when it has none this edition can read. An empty path is none. */
    private static String pathOf(Value predicate) {
        Value path = Value.get(predicate, "path");
        if (path == null || path.kind() != Value.Kind.STRING) return null;
        String text = Value.asString(path, "");
        return text.isEmpty() ? null : text;
    }

    /**
     * The third answer {@code childrenOf} can give.
     *
     * <p>Absent and non-array are deliberately different. Absent is <i>no children</i>, and a conjunction over
     * no children is satisfied — a skill declaring no preconditions does not fail them. A child list that is a
     * string is a <b>broken rule</b>, and answering "no children" there would turn it into a pass, which is the
     * failure this module exists to stop.
     */
    private static final Value BROKEN = Value.of("broken");

    private static Value childrenOf(Value predicate) {
        Value held = Value.get(predicate, "predicates");
        if (Value.nullish(held)) return null;
        if (held.kind() != Value.Kind.ARRAY) return BROKEN;
        return held;
    }

    public static Outcome evaluateOutcome(Value predicate, Value context) {
        // Not an object, or no string operator: there is no predicate here, and this is the line that makes the
        // module total.
        if (predicate == null || predicate.kind() != Value.Kind.OBJECT) {
            return unevaluable(Reason.MALFORMED, null, null);
        }
        Value opValue = Value.get(predicate, "op");
        if (opValue == null || opValue.kind() != Value.Kind.STRING) {
            return unevaluable(Reason.MALFORMED, null, null);
        }
        String op = Value.asString(opValue, "");
        String path = pathOf(predicate);

        if (op.equals("and") || op.equals("or")) {
            Value children = childrenOf(predicate);
            if (children == BROKEN) return unevaluable(Reason.MALFORMED, null, op);
            boolean conjunction = op.equals("and");
            // Order is part of the answer: a refusal names *a* predicate, and which one it names must not depend
            // on how an edition iterates.
            Outcome firstUnevaluable = null;
            for (int at = 0; at < Value.length(children); at++) {
                Outcome outcome = evaluateOutcome(Value.at(children, at), context);
                if (conjunction) {
                    if (outcome.outcome() != Kind.SATISFIED) return outcome;
                    continue;
                }
                if (outcome.outcome() == Kind.SATISFIED) return outcome;
                if (outcome.outcome() == Kind.UNEVALUABLE && firstUnevaluable == null) {
                    firstUnevaluable = outcome;
                }
            }
            // Nothing was asked for; or, for a disjunction, an alternative that held would have returned already
            // and an alternative nobody could read means this is not known to be false, only unread.
            if (conjunction) return decided(true);
            return firstUnevaluable != null ? firstUnevaluable : decided(false);
        }

        if (op.equals("not")) {
            Value children = childrenOf(predicate);
            if (children == BROKEN) return unevaluable(Reason.MALFORMED, null, op);
            Value child = Value.get(predicate, "predicate");
            if (child == null) child = Value.at(children, 0);
            if (child == null) return unevaluable(Reason.MISSING_OPERAND, null, op);
            Outcome outcome = evaluateOutcome(child, context);
            // Ignorance does not negate into knowledge, and this is the line that used to say it did.
            if (outcome.outcome() == Kind.UNEVALUABLE) return outcome;
            return decided(outcome.outcome() == Kind.UNSATISFIED);
        }

        if (op.equals("exists")) {
            if (path == null) return unevaluable(Reason.MISSING_PATH, null, op);
            // The one operator that answers *about* resolution, so it never refuses for lack of it.
            Value found = readPath(context, path);
            if (Value.nullish(found)) return decided(false);
            if (found.kind() == Value.Kind.ARRAY) return decided(Value.length(found) > 0);
            if (found.kind() == Value.Kind.STRING) {
                return decided(!Js.trim(Value.asString(found, "")).isEmpty());
            }
            return decided(true);
        }

        boolean equality = op.equals("eq") || op.equals("neq");
        boolean membership = op.equals("in") || op.equals("includes");
        boolean coverage = op.equals("includes_all") || op.equals("includes_any");
        boolean numeric = op.equals("gte") || op.equals("lte") || op.equals("gt") || op.equals("lt");
        boolean versionRange = op.equals("semver_satisfies");

        if (equality || membership || coverage || numeric || versionRange) {
            if (path == null) return unevaluable(Reason.MISSING_PATH, null, op);
            Operand left = fromPath(context, path);
            Operand right = fromValue(Value.get(predicate, "value"), context);

            // The order the two operands are checked in is the order the reference passes them, and it decides
            // which path a refusal names when both are unresolved.
            if (!left.resolved()) return unevaluable(Reason.UNRESOLVED_PATH, left.path(), op);
            if (!right.resolved()) return unevaluable(Reason.UNRESOLVED_PATH, right.path(), op);

            if (equality) {
                boolean equal = identical(left, right);
                return decided(op.equals("eq") ? equal : !equal);
            }
            if (membership) {
                // One operator reads the path as the needle and the value as the collection; the other is the
                // opposite way round. Two operators, one comparison.
                if (op.equals("in")) return decided(contains(right, left));
                return decided(contains(left, right));
            }
            if (coverage) {
                int count = operandLength(right.held());
                boolean any = false;
                boolean all = true;
                for (int at = 0; at < count; at++) {
                    if (contains(left, element(right, operandAt(right.held(), at), at))) {
                        any = true;
                    } else {
                        all = false;
                    }
                }
                // An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from a
                // requirement that *is* empty, never from one nobody could read.
                if (op.equals("includes_any")) return decided(count == 0 || any);
                return decided(all);
            }
            if (numeric) {
                Double leftNumber = toNumber(left.held());
                if (leftNumber == null) return unevaluable(Reason.NOT_A_NUMBER, left.path(), op);
                Double rightNumber = toNumber(right.held());
                if (rightNumber == null) return unevaluable(Reason.NOT_A_NUMBER, right.path(), op);
                return switch (op) {
                    case "gte" -> decided(leftNumber >= rightNumber);
                    case "lte" -> decided(leftNumber <= rightNumber);
                    case "gt" -> decided(leftNumber > rightNumber);
                    default -> decided(leftNumber < rightNumber);
                };
            }

            // A version this edition cannot parse is a fact about the declaration, not a version that fell
            // outside the window — so it is named rather than counted as a miss.
            Value version = left.held();
            if (version == null || version.kind() != Value.Kind.STRING
                    || !Semver.validVersion(Value.asString(version, ""))) {
                return unevaluable(Reason.NOT_A_VERSION, left.path(), op);
            }
            Value range = right.held();
            if (range == null || range.kind() != Value.Kind.STRING
                    || !Semver.validRange(Value.asString(range, ""))) {
                return unevaluable(Reason.NOT_A_VERSION, right.path(), op);
            }
            return decided(Semver.satisfies(Value.asString(version, ""), Value.asString(range, "")));
        }

        return unevaluable(Reason.UNKNOWN_OP, path, op);
    }

    public static Value evaluateCase(Value input) {
        Outcome outcome = evaluateOutcome(Value.get(input, "predicate"), Value.get(input, "context"));

        // The boolean and the outcome in one document, deliberately. The boolean is defined as "satisfied" of
        // the outcome, so an edition that computed the two separately would be free to disagree with itself —
        // and the whole argument for the third value is that it refines the boolean rather than offering a
        // second opinion. Freezing both makes that checkable.
        return Value.object()
                .set("satisfied", Value.of(outcome.outcome() == Kind.SATISFIED))
                .set("outcome", Value.of(outcome.outcome().label()))
                .set("reason", outcome.reason() == null ? Value.NULL : Value.of(outcome.reason().label()))
                .set("path", outcome.path() == null ? Value.NULL : Value.of(outcome.path()))
                .set("op", outcome.op() == null ? Value.NULL : Value.of(outcome.op()));
    }
}
