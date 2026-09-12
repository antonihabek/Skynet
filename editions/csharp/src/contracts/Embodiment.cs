// The embodiment descriptor and the role vocabulary — `reference/typescript/embodiment.ts`.
//
// The role vocabulary is the part of this module the whole system leans on: a role is how two bodies from two
// vendors are recognised as having the same shoulder, and §6.3's transport plan is built out of the match. The
// rule that keeps it honest is that an unrecognised role never *silently* maps to something plausible —
// `ParseRole` reports that it could not read one, and the caller treats that as "this joint can only be
// matched by exact name". A body the vocabulary does not describe is still usable; it just cannot take part in
// cross-body remapping, which is the truthful outcome, because nobody has established what its joints mean.

using System;
using System.Globalization;
using System.Text;

namespace Skynet.Os.Contracts;

/// <summary>
/// A parsed role. <c>Axis</c> is null when the role named no axis, which two segments always mean:
/// <c>torso.yaw</c> is a link called <c>yaw</c>, not an axis. <c>Index</c> is a nullable <c>double</c> so that
/// absent and zero are different — <c>base.wheel#0</c> is a valid role whose index is zero, which is the corpus
/// case <c>ordinal-zero</c> exists for, and a sentinel would read it as absent.
/// </summary>
internal sealed record Role(string Chain, string Link, string? Axis, double? Index);

internal static class Embodiment
{
    /// <summary>
    /// Twelve chains, and <c>aux</c> and <c>tail</c> are in the list so that a body nobody anticipated — a snake
    /// robot's segments, a second mast, a tail-mounted sensor boom — can still declare a role that parses,
    /// rather than being pushed into a wrong-but-close chain because the vocabulary had no honest slot for it.
    /// </summary>
    internal static readonly string[] CanonicalChains =
    {
        "left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
        "torso", "neck", "head", "base", "tail", "aux",
    };

    /// <summary>
    /// Axis of motion, when a link has more than one degree of freedom. A three-DoF shoulder is three joints in
    /// every real driver, and the axis suffix is what keeps <c>left_arm.shoulder.pitch</c> from colliding with
    /// <c>left_arm.shoulder.roll</c>.
    /// </summary>
    internal static readonly string[] CanonicalAxes =
    {
        "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
    };

    private static bool InList(string[] list, string text)
    {
        foreach (string held in list)
        {
            if (string.Equals(held, text, StringComparison.Ordinal))
            {
                return true;
            }
        }
        return false;
    }

    private static bool LowerOrUnderscore(char ch) => (ch >= 'a' && ch <= 'z') || ch == '_';

    private static bool LinkByte(char ch) => LowerOrUnderscore(ch) || (ch >= '0' && ch <= '9');

    private static bool LowerByte(char ch) => ch >= 'a' && ch <= 'z';

    private static bool DigitByte(char ch) => ch >= '0' && ch <= '9';

    private static int RunOf(string text, int at, Func<char, bool> member)
    {
        int end = at;
        while (end < text.Length && member(text[end]))
        {
            end += 1;
        }
        return end - at;
    }

    internal static Role? ParseRoleText(string role)
    {
        // `role.trim().toLowerCase()`, in that order and with the engine's full mapping: the corpus holds
        // `necK.link.pitch`, which is a chain only after lower-casing, and a role wrapped in a byte-order mark,
        // which is whitespace only to JavaScript. `string.ToLowerInvariant` is the *simple* mapping and would
        // answer differently on several characters, which is why `Js.Lower` exists.
        string text = Js.Lower(Js.Trim(role));

        // The reference's role pattern, anchored at both ends. Every character class in it is disjoint from the
        // separator that follows it, so a greedy run is the only run that can match and there is nothing to
        // backtrack: a chain of lower-case letters and underscores, a point, a link that may also hold digits,
        // an optional point and axis of lower-case letters, and an optional hash and decimal ordinal.
        int at = 0;
        int chainLength = RunOf(text, at, LowerOrUnderscore);
        if (chainLength == 0)
        {
            return null;
        }
        string chain = text.Substring(at, chainLength);
        at += chainLength;
        if (at >= text.Length || text[at] != '.')
        {
            return null;
        }
        at += 1;
        int linkLength = RunOf(text, at, LinkByte);
        if (linkLength == 0)
        {
            return null;
        }
        string link = text.Substring(at, linkLength);
        at += linkLength;

        string? axis = null;
        if (at < text.Length && text[at] == '.')
        {
            int after = at + 1;
            int axisLength = RunOf(text, after, LowerByte);
            // The axis group is optional, but a point with nothing readable after it cannot be consumed by
            // anything else either, so the whole role fails — `a.b.3` and `a.b.` both.
            if (axisLength == 0)
            {
                return null;
            }
            axis = text.Substring(after, axisLength);
            at = after + axisLength;
        }

        double? index = null;
        if (at < text.Length && text[at] == '#')
        {
            int after = at + 1;
            int indexLength = RunOf(text, after, DigitByte);
            if (indexLength == 0)
            {
                return null;
            }
            // `AsSpan` rather than `Substring`: the run has already been validated as ASCII digits, so this is
            // a conversion of bytes already in hand and there is no reason to copy them first.
            index = double.Parse(
                text.AsSpan(after, indexLength), NumberStyles.Float, CultureInfo.InvariantCulture);
            at = after + indexLength;
        }
        if (at != text.Length)
        {
            return null;
        }

        if (!InList(CanonicalChains, chain))
        {
            return null;
        }
        if (axis is not null && !InList(CanonicalAxes, axis))
        {
            return null;
        }
        return new Role(chain, link, axis, index);
    }

    /// <summary>
    /// <c>if (!role) return null</c> — a null, an absent field and an empty string are all falsy, and all three
    /// mean the same thing here: no role was declared.
    /// </summary>
    internal static Role? ParseRole(Value? role)
    {
        if (role is null || role.ValueKind != Kind.String)
        {
            return null;
        }
        string text = Value.AsString(role, string.Empty);
        return text.Length == 0 ? null : ParseRoleText(text);
    }

    /// <summary>The canonical string form, so two parsed roles can be compared as keys.</summary>
    internal static string RoleKey(Role role)
    {
        StringBuilder out_ = new();
        out_.Append(role.Chain).Append('.').Append(role.Link);
        if (role.Axis is not null)
        {
            out_.Append('.').Append(role.Axis);
        }
        if (role.Index is not null)
        {
            out_.Append('#').Append(Js.NumberToString(role.Index.Value));
        }
        return out_.ToString();
    }

    internal static Value RolesVocabularyCase(Value? input)
    {
        Role? parsed = ParseRole(Value.Get(input, "role"));
        if (parsed is null)
        {
            return Value.Null;
        }
        return Value.Object()
            .Set("chain", Value.Of(parsed.Chain))
            .Set("link", Value.Of(parsed.Link))
            .Set("axis", parsed.Axis is null ? Value.Null : Value.Of(parsed.Axis))
            .Set("index", parsed.Index is null ? Value.Null : Value.Of(parsed.Index.Value))
            .Set("key", Value.Of(RoleKey(parsed)));
    }
}
