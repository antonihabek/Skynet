// Minimal semver parsing, comparison and range satisfaction — `reference/typescript/semver.ts`.
//
// It is a contract rather than a utility because §54.2 made `semver_satisfies` an operator the robot evaluates
// before a skill may start: a robot deciding whether its runtime satisfies `^1.2.0` differently from the Hub
// that sold it the skill is the §31.2 failure, *a format with two implementations of its own definition is not
// a format*.
//
// Note what is deliberately **not** shared with it. `package.ts` compares OS versions with its own
// numeric-segment comparator, because `min_os_version` is matched against whatever an image reports, which is
// `2.0` as often as `2.0.0`. Two comparators with two jobs.
//
// And note what is not used here: `System.Version`, which accepts `1.2`, refuses a prerelease, and orders
// `1.0.0-alpha` nowhere at all. It answers a different question from this one.

using System;
using System.Collections.Generic;
using System.Globalization;

namespace Skynet.Os.Contracts;

/// <summary>
/// A parsed version, or null for anything that is not <c>major.minor.patch[-prerelease]</c>. The three
/// components are <c>double</c> because the reference's are: <c>Number(match[1])</c> over a digit run with no
/// length limit, so a version whose major is twenty digits long parses to the double nearest it and is written
/// back out that way. An <c>int</c> here would answer a different question from the oracle's on exactly that
/// input.
/// </summary>
internal sealed record Version(double Major, double Minor, double Patch, string? Prerelease, string Raw);

internal static class Semver
{
    private static bool AsciiDigit(char ch) => ch >= '0' && ch <= '9';

    /// <summary><c>[0-9A-Za-z.-]</c>, the prerelease character class.</summary>
    private static bool PrereleaseByte(char ch) =>
        AsciiDigit(ch) || (ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') || ch == '.' || ch == '-';

    /// <summary>
    /// A maximal run of ASCII digits at <c>at</c>. <c>\d</c> in the reference's regex has no <c>u</c> flag and no
    /// Unicode property, so it is ASCII 0-9 and nothing else — which is what makes the corpus case written in
    /// Arabic-Indic digits unreadable rather than a version. <c>char.IsDigit</c> would accept those, which is why
    /// it is not used.
    /// </summary>
    private static int DigitRun(string text, int at)
    {
        int end = at;
        while (end < text.Length && AsciiDigit(text[end]))
        {
            end += 1;
        }
        return end - at;
    }

    /// <summary><c>Number(...)</c> over a run of ASCII digits: the double nearest the value the digits spell.</summary>
    private static double NumberOf(string text) =>
        double.Parse(text, NumberStyles.Float, CultureInfo.InvariantCulture);

    internal static Version? Parse(string text)
    {
        string input = Js.Trim(text);

        // `^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$`, anchored at both ends. Digits and the point are
        // disjoint character classes, so a greedy run followed by a required point needs no backtracking: the
        // maximal run is the only run that can be followed by one.
        int at = 0;
        int majorLength = DigitRun(input, at);
        if (majorLength == 0)
        {
            return null;
        }
        int majorAt = at;
        at += majorLength;
        if (at >= input.Length || input[at] != '.')
        {
            return null;
        }
        at += 1;
        int minorLength = DigitRun(input, at);
        if (minorLength == 0)
        {
            return null;
        }
        int minorAt = at;
        at += minorLength;
        if (at >= input.Length || input[at] != '.')
        {
            return null;
        }
        at += 1;
        int patchLength = DigitRun(input, at);
        if (patchLength == 0)
        {
            return null;
        }
        int patchAt = at;
        at += patchLength;

        string? prerelease = null;
        if (at < input.Length)
        {
            if (input[at] != '-')
            {
                return null;
            }
            at += 1;
            int start = at;
            while (at < input.Length && PrereleaseByte(input[at]))
            {
                at += 1;
            }
            if (at == start || at != input.Length)
            {
                return null;
            }
            prerelease = input.Substring(start, at - start);
        }

        return new Version(
            NumberOf(input.Substring(majorAt, majorLength)),
            NumberOf(input.Substring(minorAt, minorLength)),
            NumberOf(input.Substring(patchAt, patchLength)),
            prerelease,
            input);
    }

    internal static bool ValidVersion(string text) => Parse(text) is not null;

    /// <summary>
    /// Is every byte of this identifier an ASCII digit? The reference's <c>/^\d+$/</c> over a <c>.</c>-separated
    /// part, which decides whether the part compares numerically or as a string.
    /// </summary>
    private static bool AllDigits(string part)
    {
        if (part.Length == 0)
        {
            return false;
        }
        foreach (char ch in part)
        {
            if (!AsciiDigit(ch))
            {
                return false;
            }
        }
        return true;
    }

    /// <summary>
    /// <c>text.split('.')[index]</c>, with null past the end — the reference's <c>undefined</c>, which the
    /// comparison reads as "this side ran out of identifiers".
    /// </summary>
    private static string? DotPart(string text, int index)
    {
        int at = 0;
        int which = 0;
        while (true)
        {
            int start = at;
            while (at < text.Length && text[at] != '.')
            {
                at += 1;
            }
            if (which == index)
            {
                return text.Substring(start, at - start);
            }
            if (at >= text.Length)
            {
                return null;
            }
            at += 1;
            which += 1;
        }
    }

    private static int DotCount(string text)
    {
        int parts = 1;
        foreach (char ch in text)
        {
            if (ch == '.')
            {
                parts += 1;
            }
        }
        return parts;
    }

    /// <summary>
    /// Prerelease identifiers per semver §11: numeric below alphanumeric, and a longer list wins a tie.
    /// <para>
    /// The string comparison is JavaScript's <c>&lt;</c>, which compares UTF-16 code units. Every byte a
    /// prerelease may hold is ASCII, so <c>string.CompareOrdinal</c> is the same comparison here — and that is a
    /// property of the grammar rather than a convenience, which is why it is written down. <c>string.Compare</c>
    /// without an ordinal comparison would order <c>alpha-1</c> and <c>alpha1</c> the other way round on a
    /// machine with ICU.
    /// </para>
    /// </summary>
    private static int ComparePrerelease(string? left, string? right)
    {
        if (left is null && right is null)
        {
            return 0;
        }
        // A version without a prerelease outranks one with it.
        if (left is null)
        {
            return 1;
        }
        if (right is null)
        {
            return -1;
        }
        if (string.Equals(left, right, StringComparison.Ordinal))
        {
            return 0;
        }

        int length = Math.Max(DotCount(left), DotCount(right));
        for (int index = 0; index < length; index++)
        {
            string? ai = DotPart(left, index);
            string? bi = DotPart(right, index);
            if (ai is null)
            {
                return -1;
            }
            if (bi is null)
            {
                return 1;
            }
            bool leftNumeric = AllDigits(ai);
            bool rightNumeric = AllDigits(bi);
            if (leftNumeric && rightNumeric)
            {
                double difference = NumberOf(ai) - NumberOf(bi);
                if (difference != 0)
                {
                    return difference < 0 ? -1 : 1;
                }
                continue;
            }
            if (leftNumeric)
            {
                return -1;
            }
            if (rightNumeric)
            {
                return 1;
            }
            int order = string.CompareOrdinal(ai, bi);
            if (order != 0)
            {
                return order < 0 ? -1 : 1;
            }
        }
        return 0;
    }

    /// <summary>
    /// -1 if a &lt; b, 0 if equal, 1 if a &gt; b — and <b>0 for anything either side could not parse</b>, which
    /// is the reference's answer and the reason <c>package.ts</c> keeps its own comparator: a strict parser in
    /// that position would silently admit a package onto an older OS.
    /// </summary>
    internal static int Compare(Version? left, Version? right)
    {
        if (left is null || right is null)
        {
            return 0;
        }
        if (left.Major != right.Major)
        {
            return left.Major < right.Major ? -1 : 1;
        }
        if (left.Minor != right.Minor)
        {
            return left.Minor < right.Minor ? -1 : 1;
        }
        if (left.Patch != right.Patch)
        {
            return left.Patch < right.Patch ? -1 : 1;
        }
        return ComparePrerelease(left.Prerelease, right.Prerelease);
    }

    // ── Ranges ───────────────────────────────────────────────────────────────

    private enum Op
    {
        Eq,
        Gte,
        Gt,
        Lte,
        Lt,
        Caret,
        Tilde,
    }

    private sealed record Operator(string Spelling, Op Op);

    /// <summary>
    /// The alternation order is the reference's and it decides two corpus cases: <c>&gt;=</c> is tried before
    /// <c>&gt;</c>, so <c>&gt;=1.0.0</c> is one comparator rather than <c>&gt;</c> applied to <c>=1.0.0</c>. A
    /// token with no operator is an equality comparator.
    /// </summary>
    private static readonly Operator[] Operators =
    {
        new(">=", Op.Gte), new("<=", Op.Lte), new(">", Op.Gt),
        new("<", Op.Lt), new("^", Op.Caret), new("~", Op.Tilde),
        new("=", Op.Eq),
    };

    private sealed record Bound(Op Op, Version Version);

    private static Operator? OperatorAt(string text, int at)
    {
        foreach (Operator candidate in Operators)
        {
            if (text.Length - at >= candidate.Spelling.Length
                && string.CompareOrdinal(text, at, candidate.Spelling, 0, candidate.Spelling.Length) == 0)
            {
                return candidate;
            }
        }
        return null;
    }

    /// <summary>
    /// <c>\s*</c> — JavaScript's whitespace class, which is the same set <c>trim()</c> removes and includes
    /// U+FEFF. A range written with one before its version is a corpus case.
    /// </summary>
    private static int SkipSpace(string text, int at)
    {
        int scan = at;
        while (scan < text.Length)
        {
            char unit = text[scan];
            bool pair = char.IsHighSurrogate(unit) && scan + 1 < text.Length
                && char.IsLowSurrogate(text[scan + 1]);
            // The code unit itself when there is no pair, which is what `codePointAt` answers for a lone
            // surrogate — no whitespace character is above the BMP, so this is only about not throwing on one.
            int code = pair ? char.ConvertToUtf32(unit, text[scan + 1]) : unit;
            if (!Js.IsSpace(code))
            {
                break;
            }
            scan += pair ? 2 : 1;
        }
        return scan;
    }

    /// <summary>
    /// One token of the reference's tokenizer:
    /// <c>/(?:&gt;=|&lt;=|&gt;|&lt;|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?/g</c>
    /// <para>
    /// -1 when nothing matches at <c>at</c>, which is when the caller advances one character and tries again —
    /// the way a global regex walks a string. Tokenizing on the operator rather than on whitespace is what makes
    /// <c>&gt;= 1.2.0</c> and <c>&gt;=1.2.0</c> the same range, and <c>&gt;=1.0.0&lt;2.0.0</c> two comparators.
    /// </para>
    /// </summary>
    private static int TokenAt(string text, int at)
    {
        int scan = at;
        Operator? found = OperatorAt(text, scan);
        if (found is not null)
        {
            scan += found.Spelling.Length;
        }
        scan = SkipSpace(text, scan);
        int major = DigitRun(text, scan);
        if (major == 0)
        {
            return -1;
        }
        scan += major;
        if (scan >= text.Length || text[scan] != '.')
        {
            return -1;
        }
        scan += 1;
        int minor = DigitRun(text, scan);
        if (minor == 0)
        {
            return -1;
        }
        scan += minor;
        if (scan >= text.Length || text[scan] != '.')
        {
            return -1;
        }
        scan += 1;
        int patch = DigitRun(text, scan);
        if (patch == 0)
        {
            return -1;
        }
        scan += patch;
        if (scan < text.Length && text[scan] == '-')
        {
            int start = scan + 1;
            int run = start;
            while (run < text.Length && PrereleaseByte(text[run]))
            {
                run += 1;
            }
            // The prerelease group is optional, so a lone `-` with nothing readable after it simply is not part
            // of the token rather than failing it.
            if (run > start)
            {
                scan = run;
            }
        }
        return scan;
    }

    private static Bound? ParseBound(string token)
    {
        string trimmed = Js.Trim(token);
        int at = 0;
        Op op = Op.Eq;
        Operator? found = OperatorAt(trimmed, at);
        if (found is not null)
        {
            op = found.Op;
            at += found.Spelling.Length;
        }
        at = SkipSpace(trimmed, at);
        // `(.+)` requires at least one character, and `Parse` trims again.
        if (at >= trimmed.Length)
        {
            return null;
        }
        Version? version = Parse(trimmed.Substring(at));
        return version is null ? null : new Bound(op, version);
    }

    private static bool SatisfiesBound(Version target, Bound against)
    {
        int order = Compare(target, against.Version);
        switch (against.Op)
        {
            case Op.Gte: return order >= 0;
            case Op.Gt: return order > 0;
            case Op.Lte: return order <= 0;
            case Op.Lt: return order < 0;
            case Op.Eq: return order == 0;
            case Op.Caret:
                // Caret: compatible within the leftmost non-zero component.
                if (order < 0)
                {
                    return false;
                }
                Version base_ = against.Version;
                if (base_.Major > 0)
                {
                    return target.Major == base_.Major;
                }
                if (base_.Minor > 0)
                {
                    return target.Major == 0 && target.Minor == base_.Minor;
                }
                return target.Major == 0 && target.Minor == 0 && target.Patch == base_.Patch;
            default:
                // Tilde: patch-level changes within the same minor.
                return order >= 0 && target.Major == against.Version.Major
                    && target.Minor == against.Version.Minor;
        }
    }

    /// <summary>
    /// A group of AND comparators. <c>Readable</c> is false when a token the tokenizer produced was refused by
    /// the comparator parser, which makes the whole group unreadable — exactly as
    /// <c>comparators.some(c =&gt; c === null)</c> does.
    /// </summary>
    private sealed record Group(bool Readable, List<Bound> Bounds);

    private sealed record ParsedRange(bool Wildcard, List<Group> Groups);

    /// <summary>
    /// A range, parsed into its <c>||</c> groups.
    /// <para>
    /// A group that does not parse is marked unreadable rather than failing the whole range, which is what keeps
    /// one unreadable alternative from invalidating the readable ones — and <c>ValidRange</c> is the caller that
    /// wants the stricter answer, which it gets by asking about every group.
    /// </para>
    /// </summary>
    private static ParsedRange ParseRange(string range)
    {
        string trimmed = Js.Trim(range);
        if (trimmed.Length == 0 || string.Equals(trimmed, "*", StringComparison.Ordinal))
        {
            return new ParsedRange(true, new List<Group>());
        }
        if (string.Equals(Js.Lower(trimmed), "any", StringComparison.Ordinal))
        {
            return new ParsedRange(true, new List<Group>());
        }

        List<Group> groups = new();
        // `split('||')`, which yields one more piece than there are separators and can yield empty ones.
        // `string.Split` keeps the empties unless told otherwise, which is what is wanted here.
        foreach (string raw in trimmed.Split("||", StringSplitOptions.None))
        {
            string piece = Js.Trim(raw);
            List<Bound> bounds = new();
            bool unreadable = false;
            // The global tokenizer: at each position, match or advance one character.
            int scan = 0;
            while (scan < piece.Length)
            {
                int end = TokenAt(piece, scan);
                if (end < 0)
                {
                    scan += 1;
                    continue;
                }
                Bound? held = ParseBound(piece.Substring(scan, end - scan));
                if (held is null)
                {
                    unreadable = true;
                    break;
                }
                bounds.Add(held);
                scan = end;
            }
            groups.Add(new Group(!unreadable && bounds.Count != 0, bounds));
        }
        return new ParsedRange(false, groups);
    }

    internal static bool Satisfies(string version, string range)
    {
        Version? target = Parse(version);
        if (target is null)
        {
            return false;
        }
        ParsedRange parsed = ParseRange(range);
        if (parsed.Wildcard)
        {
            return true;
        }
        foreach (Group group in parsed.Groups)
        {
            if (!group.Readable)
            {
                continue;
            }
            bool all = true;
            foreach (Bound held in group.Bounds)
            {
                if (!SatisfiesBound(target, held))
                {
                    all = false;
                    break;
                }
            }
            if (all)
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>
    /// Is <c>range</c> a range this edition can read at all?
    /// <para>
    /// <c>Satisfies</c> answers false both for "this version is outside that window" and for "that is not a
    /// window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is a skill
    /// nobody can ever run here, and its publisher is the one who has to hear about it.
    /// </para>
    /// </summary>
    internal static bool ValidRange(string range)
    {
        ParsedRange parsed = ParseRange(range);
        if (parsed.Wildcard)
        {
            return true;
        }
        foreach (Group group in parsed.Groups)
        {
            if (!group.Readable)
            {
                return false;
            }
        }
        return true;
    }

    // ── The shapes, from `reference/typescript/shape.ts` ──────────────────────

    private static Value ShapeVersion(Version? version)
    {
        if (version is null)
        {
            return Value.Null;
        }
        return Value.Object()
            .Set("major", Value.Of(version.Major))
            .Set("minor", Value.Of(version.Minor))
            .Set("patch", Value.Of(version.Patch))
            .Set("prerelease", version.Prerelease is null ? Value.Null : Value.Of(version.Prerelease))
            .Set("raw", Value.Of(version.Raw));
    }

    internal static Value SatisfiesCase(Value? input)
    {
        string version = Value.AsString(Value.Get(input, "version"), string.Empty);
        string range = Value.AsString(Value.Get(input, "range"), string.Empty);
        return Value.Object()
            .Set("version", Value.Of(version))
            .Set("range", Value.Of(range))
            .Set("satisfies", Value.Of(Satisfies(version, range)))
            // Validity beside satisfaction, because the two are different facts: pinning only the boolean would
            // let an edition agree about every satisfaction and disagree about which ranges it could read at all
            // — the difference between refusing a skill and telling its publisher why.
            .Set("valid_version", Value.Of(ValidVersion(version)))
            .Set("valid_range", Value.Of(ValidRange(range)));
    }

    internal static Value CompareCase(Value? input)
    {
        Version? left = Parse(Value.AsString(Value.Get(input, "a"), string.Empty));
        Version? right = Parse(Value.AsString(Value.Get(input, "b"), string.Empty));
        return Value.Object()
            .Set("compare", Value.Of((double)Compare(left, right)))
            .Set("a", ShapeVersion(left))
            .Set("b", ShapeVersion(right));
    }
}
