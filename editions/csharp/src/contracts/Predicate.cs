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
// So there is one evaluator, and the boolean is `Outcome == Satisfied` of it. Two functions computing the same
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

using System;
using System.Globalization;

namespace Skynet.Os.Contracts;

internal enum Decision
{
    Satisfied,
    Unsatisfied,
    Unevaluable,
}

/// <summary>
/// Why a predicate could not be decided.
/// <para>
/// A closed vocabulary, because §54.2 requires the refusal to <i>name</i> something and a free-text reason is
/// the same nothing as a free-text precondition. <c>corpus/vocabulary/closed-sets.json</c> holds every edition
/// to this list.
/// </para>
/// </summary>
internal enum Reason
{
    /// <summary>Not an object, or the operator is not a string. There is no predicate here.</summary>
    Malformed,

    /// <summary>An operator this edition does not implement.</summary>
    UnknownOp,

    /// <summary>The predicate names no path to read.</summary>
    MissingPath,

    /// <summary>A negation with nothing to negate.</summary>
    MissingOperand,

    /// <summary>A path that names nothing in this context. This is the one §54.2 is about.</summary>
    UnresolvedPath,

    /// <summary>An operand is present but is not a finite number, and the operator compares numbers.</summary>
    NotANumber,

    /// <summary>An operand is present but is not a version, or not a range.</summary>
    NotAVersion,
}

internal sealed record Outcome(Decision Decision, Reason? Reason, string? Path, string? Op);

internal static class Predicate
{
    /// <summary>
    /// The wire spelling of a decision and of a reason. Written out rather than derived from the enum name,
    /// because <c>Enum.ToString().ToLowerInvariant()</c> would give <c>unknownop</c> for <c>UnknownOp</c> and the
    /// vocabulary corpus pins <c>unknown_op</c> — a name-derived spelling would be a second definition of the
    /// closed set, in a language whose casing convention differs from the set's.
    /// </summary>
    private static string Label(Decision decision) => decision switch
    {
        Decision.Satisfied => "satisfied",
        Decision.Unsatisfied => "unsatisfied",
        _ => "unevaluable",
    };

    private static string Label(Reason reason) => reason switch
    {
        Reason.Malformed => "malformed",
        Reason.UnknownOp => "unknown_op",
        Reason.MissingPath => "missing_path",
        Reason.MissingOperand => "missing_operand",
        Reason.UnresolvedPath => "unresolved_path",
        Reason.NotANumber => "not_a_number",
        _ => "not_a_version",
    };

    private static Outcome Decided(bool truth) =>
        new(truth ? Decision.Satisfied : Decision.Unsatisfied, null, null, null);

    private static Outcome Unevaluable(Reason reason, string? path, string? op) =>
        new(Decision.Unevaluable, reason, path, op);

    /// <summary>
    /// A canonical decimal index, which is what a JSON array member is named: zero, or a non-zero digit followed
    /// by any digits. So <c>01</c>, <c>1.0</c> and <c>-1</c> are not members of anything, and -1 is returned for a
    /// segment that is not an index.
    /// </summary>
    private static int CanonicalIndex(string segment)
    {
        if (segment.Length == 0)
        {
            return -1;
        }
        if (segment[0] == '0')
        {
            return segment.Length == 1 ? 0 : -1;
        }
        if (segment[0] < '1' || segment[0] > '9')
        {
            return -1;
        }
        long out_ = 0;
        foreach (char ch in segment)
        {
            if (ch < '0' || ch > '9')
            {
                return -1;
            }
            out_ = (out_ * 10) + (ch - '0');
            // A path naming an index beyond any array this document could hold resolves to nothing, which is the
            // same answer as an index past the end.
            if (out_ > int.MaxValue)
            {
                return -1;
            }
        }
        return (int)out_;
    }

    /// <summary>
    /// Read a dotted path out of a context. Null — the reference's <c>undefined</c> — for any missing link.
    /// <para>
    /// <b>Members only.</b> An array segment must be a canonical decimal index, so a path ending in
    /// <c>length</c> does not resolve: that is a property of a JavaScript array rather than a member of the JSON
    /// document, and an edition without JavaScript arrays could not reproduce it. The same restriction keeps a
    /// path off the prototype chain, where a path composed by somebody else has no business going.
    /// </para>
    /// </summary>
    internal static Value? ReadPath(Value? context, string? path)
    {
        if (string.IsNullOrEmpty(path))
        {
            return null;
        }
        Value? current = context;
        int at = 0;
        while (true)
        {
            int start = at;
            while (at < path.Length && path[at] != '.')
            {
                at += 1;
            }
            string segment = path.Substring(start, at - start);

            // A null or absent link, and a link that is not an object or an array, both answer undefined: a path
            // cannot descend into a string, a number or a boolean.
            if (current is null)
            {
                return null;
            }
            if (current.ValueKind == Kind.Array)
            {
                int index = CanonicalIndex(segment);
                if (index < 0)
                {
                    return null;
                }
                // Past the end is undefined, which is null here — not a null member.
                current = Value.At(current, index);
            }
            else if (current.ValueKind == Kind.Object)
            {
                // An own-property test, so a member present and null resolves to null and a member that is
                // absent resolves to undefined. The two are different facts throughout.
                if (!Value.Has(current, segment))
                {
                    return null;
                }
                current = Value.Get(current, segment);
            }
            else
            {
                return null;
            }

            if (at >= path.Length)
            {
                return current;
            }
            at += 1;
        }
    }

    /// <summary>
    /// One side of a comparison.
    /// <para>
    /// <c>Path</c> is where the value came from, or null for a literal, and it is load-bearing twice: it is what
    /// a refusal names, and it is how <c>Identical</c> decides object equality without a reference to compare.
    /// <c>Resolved</c> is false when the path named nothing; a literal is always resolved, even a literal null.
    /// </para>
    /// </summary>
    private sealed record Operand(Value? Held, string? Path, bool Resolved);

    /// <summary>The path a rule value references, or null when it is a literal.</summary>
    private static string? ReferencedPath(Value? held)
    {
        if (held is null || held.ValueKind != Kind.Object)
        {
            return null;
        }
        Value? reference = Value.Get(held, "__ref");
        if (reference is not null && reference.ValueKind == Kind.String)
        {
            return Value.AsString(reference, string.Empty);
        }
        // A bare `path` object is accepted as an alias so hand-written rules read naturally — and only when it is
        // the *whole* object, so a predicate carrying both a path and an operator is not one.
        Value? alias = Value.Get(held, "path");
        if (alias is not null && alias.ValueKind == Kind.String && Value.Length(held) == 1)
        {
            return Value.AsString(alias, string.Empty);
        }
        return null;
    }

    private static Operand FromPath(Value? context, string? path)
    {
        Value? held = ReadPath(context, path);
        // Resolved means "not undefined" — a path that resolved to an explicit null *is* resolved.
        return new Operand(held, path, held is not null);
    }

    private static Operand FromValue(Value? held, Value? context)
    {
        string? reference = ReferencedPath(held);
        return reference is null ? new Operand(held, null, true) : FromPath(context, reference);
    }

    /// <summary>
    /// An element of a collection operand, which is a member of the context in its own right — and so has a path
    /// of its own, which is what makes two elements of one array distinguishable to <c>Identical</c>.
    /// </summary>
    private static Operand Element(Operand collection, Value? item, int index) => new(
        item,
        collection.Path is null ? null : collection.Path + "." + index.ToString(CultureInfo.InvariantCulture),
        true);

    private static bool Primitive(Value? held) =>
        held is null || (held.ValueKind != Kind.Array && held.ValueKind != Kind.Object);

    /// <summary>
    /// Strict equality over two primitives, including the two absences: undefined equals undefined, null equals
    /// null, and the two are not each other. NaN equals nothing, itself included.
    /// </summary>
    private static bool StrictEqual(Value? left, Value? right)
    {
        if (left is null || right is null)
        {
            return left is null && right is null;
        }
        if (left.ValueKind != right.ValueKind)
        {
            return false;
        }
        switch (left.ValueKind)
        {
            case Kind.Null: return true;
            case Kind.Bool: return Value.AsBool(left, false) == Value.AsBool(right, true);
            // A primitive comparison rather than `double.Equals`, deliberately: NaN must not equal itself and
            // negative zero must equal zero, and `double.Equals` answers the opposite on both.
            case Kind.Number: return Value.AsNumber(left, 0) == Value.AsNumber(right, 1);
            case Kind.String:
                return string.Equals(
                    Value.AsString(left, string.Empty), Value.AsString(right, " "), StringComparison.Ordinal);
            default: return false;
        }
    }

    /// <summary>
    /// Strict equality on two objects is reference equality in JavaScript, and the reference is the
    /// specification, so the rule other editions must reproduce is written here rather than left to the host
    /// language.
    /// <para>
    /// Two operands are identical when both are primitives and equal, or when both were read from the <b>same
    /// path of the same context</b>, which is the only way two non-primitive operands can be one object. A
    /// literal object or array in a rule is therefore never equal to anything — which is exactly what strict
    /// equality does, and is worth knowing before writing a rule whose value is a one-element array and
    /// wondering why it never fires. Membership is the operator for that.
    /// </para>
    /// </summary>
    private static bool Identical(Operand left, Operand right)
    {
        if (Primitive(left.Held) && Primitive(right.Held))
        {
            return StrictEqual(left.Held, right.Held);
        }
        if (left.Path is null || right.Path is null)
        {
            return false;
        }
        return string.Equals(left.Path, right.Path, StringComparison.Ordinal);
    }

    /// <summary>An array is itself, an absence is empty, and anything else is a list of one.</summary>
    private static int OperandLength(Value? held)
    {
        if (held is null)
        {
            return 0;
        }
        if (held.ValueKind == Kind.Array)
        {
            return Value.Length(held);
        }
        return held.ValueKind == Kind.Null ? 0 : 1;
    }

    private static Value? OperandAt(Value? held, int index)
    {
        if (held is null)
        {
            return null;
        }
        return held.ValueKind == Kind.Array ? Value.At(held, index) : held;
    }

    /// <summary>Membership, with <c>Identical</c>'s rule applied element by element.</summary>
    private static bool Contains(Operand collection, Operand needle)
    {
        int count = OperandLength(collection.Held);
        for (int at = 0; at < count; at++)
        {
            if (Identical(Element(collection, OperandAt(collection.Held, at), at), needle))
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>
    /// A finite number is itself, a non-blank string is converted, and everything else — a boolean, an array, an
    /// absence, a blank string — is not a number. Null when it is not.
    /// </summary>
    private static double? ToNumber(Value? held)
    {
        if (held is null)
        {
            return null;
        }
        if (held.ValueKind == Kind.Number)
        {
            double number = Value.AsNumber(held, 0);
            return double.IsFinite(number) ? number : null;
        }
        if (held.ValueKind == Kind.String)
        {
            string text = Value.AsString(held, string.Empty);
            // A blank string converts to zero, and the reference refuses it before converting: a robot that
            // reported three spaces for its battery has not reported a number.
            if (Js.Trim(text).Length == 0)
            {
                return null;
            }
            double parsed = Js.StringToNumber(text);
            return double.IsFinite(parsed) ? parsed : null;
        }
        return null;
    }

    /// <summary>The predicate's own path, or null when it has none this edition can read. An empty path is none.</summary>
    private static string? PathOf(Value? predicate)
    {
        Value? path = Value.Get(predicate, "path");
        if (path is null || path.ValueKind != Kind.String)
        {
            return null;
        }
        string text = Value.AsString(path, string.Empty);
        return text.Length == 0 ? null : text;
    }

    /// <summary>
    /// The third answer <c>ChildrenOf</c> can give.
    /// <para>
    /// Absent and non-array are deliberately different. Absent is <i>no children</i>, and a conjunction over no
    /// children is satisfied — a skill declaring no preconditions does not fail them. A child list that is a
    /// string is a <b>broken rule</b>, and answering "no children" there would turn it into a pass, which is the
    /// failure this module exists to stop.
    /// </para>
    /// </summary>
    private static readonly Value Broken = Value.Of("broken");

    private static Value? ChildrenOf(Value? predicate)
    {
        Value? held = Value.Get(predicate, "predicates");
        if (Value.Nullish(held))
        {
            return null;
        }
        return held!.ValueKind == Kind.Array ? held : Broken;
    }

    internal static Outcome EvaluateOutcome(Value? predicate, Value? context)
    {
        // Not an object, or no string operator: there is no predicate here, and this is the line that makes the
        // module total.
        if (predicate is null || predicate.ValueKind != Kind.Object)
        {
            return Unevaluable(Reason.Malformed, null, null);
        }
        Value? opValue = Value.Get(predicate, "op");
        if (opValue is null || opValue.ValueKind != Kind.String)
        {
            return Unevaluable(Reason.Malformed, null, null);
        }
        string op = Value.AsString(opValue, string.Empty);
        string? path = PathOf(predicate);

        if (string.Equals(op, "and", StringComparison.Ordinal) || string.Equals(op, "or", StringComparison.Ordinal))
        {
            Value? children = ChildrenOf(predicate);
            if (ReferenceEquals(children, Broken))
            {
                return Unevaluable(Reason.Malformed, null, op);
            }
            bool conjunction = string.Equals(op, "and", StringComparison.Ordinal);
            // Order is part of the answer: a refusal names *a* predicate, and which one it names must not depend
            // on how an edition iterates.
            Outcome? firstUnevaluable = null;
            int count = Value.Length(children);
            for (int at = 0; at < count; at++)
            {
                Outcome outcome = EvaluateOutcome(Value.At(children, at), context);
                if (conjunction)
                {
                    if (outcome.Decision != Decision.Satisfied)
                    {
                        return outcome;
                    }
                    continue;
                }
                if (outcome.Decision == Decision.Satisfied)
                {
                    return outcome;
                }
                if (outcome.Decision == Decision.Unevaluable && firstUnevaluable is null)
                {
                    firstUnevaluable = outcome;
                }
            }
            // Nothing was asked for; or, for a disjunction, an alternative that held would have returned already
            // and an alternative nobody could read means this is not known to be false, only unread.
            if (conjunction)
            {
                return Decided(true);
            }
            return firstUnevaluable ?? Decided(false);
        }

        if (string.Equals(op, "not", StringComparison.Ordinal))
        {
            Value? children = ChildrenOf(predicate);
            if (ReferenceEquals(children, Broken))
            {
                return Unevaluable(Reason.Malformed, null, op);
            }
            Value? child = Value.Get(predicate, "predicate") ?? Value.At(children, 0);
            if (child is null)
            {
                return Unevaluable(Reason.MissingOperand, null, op);
            }
            Outcome outcome = EvaluateOutcome(child, context);
            // Ignorance does not negate into knowledge, and this is the line that used to say it did.
            if (outcome.Decision == Decision.Unevaluable)
            {
                return outcome;
            }
            return Decided(outcome.Decision == Decision.Unsatisfied);
        }

        if (string.Equals(op, "exists", StringComparison.Ordinal))
        {
            if (path is null)
            {
                return Unevaluable(Reason.MissingPath, null, op);
            }
            // The one operator that answers *about* resolution, so it never refuses for lack of it.
            Value? found = ReadPath(context, path);
            if (Value.Nullish(found))
            {
                return Decided(false);
            }
            if (found!.ValueKind == Kind.Array)
            {
                return Decided(Value.Length(found) > 0);
            }
            if (found.ValueKind == Kind.String)
            {
                return Decided(Js.Trim(Value.AsString(found, string.Empty)).Length != 0);
            }
            return Decided(true);
        }

        bool equality = string.Equals(op, "eq", StringComparison.Ordinal)
            || string.Equals(op, "neq", StringComparison.Ordinal);
        bool membership = string.Equals(op, "in", StringComparison.Ordinal)
            || string.Equals(op, "includes", StringComparison.Ordinal);
        bool coverage = string.Equals(op, "includes_all", StringComparison.Ordinal)
            || string.Equals(op, "includes_any", StringComparison.Ordinal);
        bool numeric = string.Equals(op, "gte", StringComparison.Ordinal)
            || string.Equals(op, "lte", StringComparison.Ordinal)
            || string.Equals(op, "gt", StringComparison.Ordinal)
            || string.Equals(op, "lt", StringComparison.Ordinal);
        bool versionRange = string.Equals(op, "semver_satisfies", StringComparison.Ordinal);

        if (equality || membership || coverage || numeric || versionRange)
        {
            if (path is null)
            {
                return Unevaluable(Reason.MissingPath, null, op);
            }
            Operand left = FromPath(context, path);
            Operand right = FromValue(Value.Get(predicate, "value"), context);

            // The order the two operands are checked in is the order the reference passes them, and it decides
            // which path a refusal names when both are unresolved.
            if (!left.Resolved)
            {
                return Unevaluable(Reason.UnresolvedPath, left.Path, op);
            }
            if (!right.Resolved)
            {
                return Unevaluable(Reason.UnresolvedPath, right.Path, op);
            }

            if (equality)
            {
                bool equal = Identical(left, right);
                return Decided(string.Equals(op, "eq", StringComparison.Ordinal) ? equal : !equal);
            }
            if (membership)
            {
                // One operator reads the path as the needle and the value as the collection; the other is the
                // opposite way round. Two operators, one comparison.
                return string.Equals(op, "in", StringComparison.Ordinal)
                    ? Decided(Contains(right, left))
                    : Decided(Contains(left, right));
            }
            if (coverage)
            {
                int count = OperandLength(right.Held);
                bool any = false;
                bool all = true;
                for (int at = 0; at < count; at++)
                {
                    if (Contains(left, Element(right, OperandAt(right.Held, at), at)))
                    {
                        any = true;
                    }
                    else
                    {
                        all = false;
                    }
                }
                // An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from a
                // requirement that *is* empty, never from one nobody could read.
                if (string.Equals(op, "includes_any", StringComparison.Ordinal))
                {
                    return Decided(count == 0 || any);
                }
                return Decided(all);
            }
            if (numeric)
            {
                double? leftNumber = ToNumber(left.Held);
                if (leftNumber is null)
                {
                    return Unevaluable(Reason.NotANumber, left.Path, op);
                }
                double? rightNumber = ToNumber(right.Held);
                if (rightNumber is null)
                {
                    return Unevaluable(Reason.NotANumber, right.Path, op);
                }
                return op switch
                {
                    "gte" => Decided(leftNumber.Value >= rightNumber.Value),
                    "lte" => Decided(leftNumber.Value <= rightNumber.Value),
                    "gt" => Decided(leftNumber.Value > rightNumber.Value),
                    _ => Decided(leftNumber.Value < rightNumber.Value),
                };
            }

            // A version this edition cannot parse is a fact about the declaration, not a version that fell
            // outside the window — so it is named rather than counted as a miss.
            Value? version = left.Held;
            if (version is null || version.ValueKind != Kind.String
                || !Semver.ValidVersion(Value.AsString(version, string.Empty)))
            {
                return Unevaluable(Reason.NotAVersion, left.Path, op);
            }
            Value? range = right.Held;
            if (range is null || range.ValueKind != Kind.String
                || !Semver.ValidRange(Value.AsString(range, string.Empty)))
            {
                return Unevaluable(Reason.NotAVersion, right.Path, op);
            }
            return Decided(Semver.Satisfies(
                Value.AsString(version, string.Empty), Value.AsString(range, string.Empty)));
        }

        return Unevaluable(Reason.UnknownOp, path, op);
    }

    internal static Value EvaluateCase(Value? input)
    {
        Outcome outcome = EvaluateOutcome(Value.Get(input, "predicate"), Value.Get(input, "context"));

        // The boolean and the outcome in one document, deliberately. The boolean is defined as "satisfied" of the
        // outcome, so an edition that computed the two separately would be free to disagree with itself — and the
        // whole argument for the third value is that it refines the boolean rather than offering a second
        // opinion. Freezing both makes that checkable.
        return Value.Object()
            .Set("satisfied", Value.Of(outcome.Decision == Decision.Satisfied))
            .Set("outcome", Value.Of(Label(outcome.Decision)))
            .Set("reason", outcome.Reason is null ? Value.Null : Value.Of(Label(outcome.Reason.Value)))
            .Set("path", outcome.Path is null ? Value.Null : Value.Of(outcome.Path))
            .Set("op", outcome.Op is null ? Value.Null : Value.Of(outcome.Op));
    }
}
