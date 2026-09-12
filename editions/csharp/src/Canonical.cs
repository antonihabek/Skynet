// The canonical writer.
//
// corpus/README.md's five rules. Two of them are where .NET's standard library is closest to right and still
// not it, and the difference between the two cases is worth keeping straight:
//
// # Rule 3: the digits are right and the shape is not
//
// `double.ToString("R")` on .NET Core 3.0 and later **is** the shortest decimal that round-trips, and unlike
// Java's `Double.toString` it is shortest even for the smallest subnormal — it writes `5E-324`, which is the
// one-digit answer the reference's rule produces. That is worth stating because it means this edition does not
// need the precision loop the C and Java editions run.
//
// What it gets wrong is the shape, in two places rule 3 names explicitly:
//
// * **Exponential wherever it likes** — `1E-07`, `1E+21` — and rule 3 is positional always.
// * **`-0`**, and rule 3 writes `0`.
//
// So the digits come from the library and the assembly is ours.
//
// # Rule 2 is not `CompareOrdinal`
//
// Keys sort by their UTF-8 **bytes**; `string.CompareOrdinal` compares UTF-16 code units. The two agree for
// every key in the basic multilingual plane and disagree above it, because a surrogate pair sorts below
// U+E000…U+FFFF as code units and above it as UTF-8.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace Skynet.Os;

public sealed class NonFiniteNumberException : Exception
{
    public NonFiniteNumberException(double value)
        : base(string.Create(CultureInfo.InvariantCulture, $"cannot serialise a non-finite number: {value}"))
    {
    }

    public NonFiniteNumberException()
        : base("cannot serialise a non-finite number")
    {
    }

    public NonFiniteNumberException(string message)
        : base(message)
    {
    }

    public NonFiniteNumberException(string message, Exception inner)
        : base(message, inner)
    {
    }
}

internal static class Canonical
{
    /// <summary>Turn <c>-4.4E-01</c> into <c>-0.44</c>.</summary>
    /// <remarks>
    /// The reference's <c>exponentialToPositional</c>. The exponent is read as a number, so .NET's two-digit
    /// minimum and JavaScript's one-digit rendering are the same input to this.
    /// </remarks>
    private static string ExponentialToPositional(string rendered)
    {
        int marker = rendered.IndexOf('E', StringComparison.Ordinal);
        if (marker < 0)
        {
            return rendered;
        }

        string mantissa = rendered[..marker];
        int exponent = int.Parse(rendered[(marker + 1)..], CultureInfo.InvariantCulture);
        bool negative = mantissa.StartsWith('-');
        if (negative)
        {
            mantissa = mantissa[1..];
        }
        string digits = mantissa.Replace(".", string.Empty, StringComparison.Ordinal);

        StringBuilder out_ = new();
        if (negative)
        {
            out_.Append('-');
        }
        // The first digit carries weight 10^exponent, so the point falls after `exponent + 1` digits.
        int point = exponent + 1;
        if (point <= 0)
        {
            out_.Append("0.").Append('0', -point).Append(digits);
        }
        else if (point >= digits.Length)
        {
            out_.Append(digits).Append('0', point - digits.Length);
        }
        else
        {
            out_.Append(digits, 0, point).Append('.').Append(digits[point..]);
        }
        return out_.ToString();
    }

    /// <summary>Format a number in canonical form. Throws on a non-finite value rather than emitting null.</summary>
    internal static string FormatNumber(double value)
    {
        if (!double.IsFinite(value))
        {
            throw new NonFiniteNumberException(value);
        }
        // Covers -0.0, which rule 3 writes as `0` and `ToString("R")` writes as `-0`. Comparing against 0.0
        // rather than testing the sign bit is deliberate: -0.0 == 0.0 is true, and that is the whole rule.
        if (value == 0.0)
        {
            return "0";
        }
        // The digits are the library's — `"R"` is shortest-round-trip here, including for the smallest
        // subnormal, which is the one place Java's equivalent is not. The shape is this function's.
        return ExponentialToPositional(value.ToString("R", CultureInfo.InvariantCulture));
    }

    /// <summary>
    /// Rule 4: escape the quote, the backslash and the C0 controls, and nothing else.
    /// </summary>
    /// <remarks>
    /// Iterated by UTF-16 unit rather than by code point on purpose: a surrogate pair is two units and neither
    /// is a control, so each is appended unchanged and the pair survives — which is also what a lone surrogate
    /// needs, since a JavaScript string can hold one and the reference can therefore produce one.
    /// </remarks>
    private static void WriteString(string text, StringBuilder out_)
    {
        out_.Append('"');
        foreach (char ch in text)
        {
            switch (ch)
            {
                case '"': out_.Append("\\\""); break;
                case '\\': out_.Append("\\\\"); break;
                case '\b': out_.Append("\\b"); break;
                case '\f': out_.Append("\\f"); break;
                case '\n': out_.Append("\\n"); break;
                case '\r': out_.Append("\\r"); break;
                case '\t': out_.Append("\\t"); break;
                default:
                    if (ch < 0x20)
                    {
                        out_.Append(string.Create(CultureInfo.InvariantCulture, $"\\u{(int)ch:x4}"));
                    }
                    else
                    {
                        out_.Append(ch);
                    }
                    break;
            }
        }
        out_.Append('"');
    }

    /// <summary>
    /// Rule 2: keys ascending by their UTF-8 byte sequence.
    /// </summary>
    /// <remarks>
    /// Compared as encoded bytes rather than with <c>CompareOrdinal</c>, which compares UTF-16 code units and
    /// disagrees for any key above the basic multilingual plane.
    /// </remarks>
    private static int CompareUtf8(string left, string right)
    {
        byte[] a = Encoding.UTF8.GetBytes(left);
        byte[] b = Encoding.UTF8.GetBytes(right);
        int shared = Math.Min(a.Length, b.Length);
        for (int at = 0; at < shared; at++)
        {
            int difference = a[at] - b[at];
            if (difference != 0)
            {
                return difference;
            }
        }
        return a.Length - b.Length;
    }

    private static void WriteValue(Value? value, StringBuilder out_)
    {
        if (value is null || value.ValueKind == Kind.Null)
        {
            out_.Append("null");
            return;
        }
        switch (value.ValueKind)
        {
            case Kind.Bool:
                out_.Append(Value.AsBool(value, false) ? "true" : "false");
                break;
            case Kind.Number:
                out_.Append(FormatNumber(value.Number));
                break;
            case Kind.String:
                WriteString(value.Text, out_);
                break;
            case Kind.Array:
                out_.Append('[');
                for (int at = 0; at < value.Items.Count; at++)
                {
                    if (at > 0)
                    {
                        out_.Append(',');
                    }
                    WriteValue(value.Items[at], out_);
                }
                out_.Append(']');
                break;
            case Kind.Object:
                List<Member> sorted = new(value.Members);
                // A stable sort is not needed — an object cannot hold two equal keys — but `List.Sort` is
                // unstable and `OrderBy` is stable, and using the stable one costs nothing and removes the
                // question.
                sorted.Sort((left, right) => CompareUtf8(left.Key, right.Key));
                out_.Append('{');
                for (int at = 0; at < sorted.Count; at++)
                {
                    if (at > 0)
                    {
                        out_.Append(',');
                    }
                    WriteString(sorted[at].Key, out_);
                    out_.Append(':');
                    WriteValue(sorted[at].Value, out_);
                }
                out_.Append('}');
                break;
            default:
                out_.Append("null");
                break;
        }
    }

    /// <summary>Serialise a value in canonical form.</summary>
    internal static string Encode(Value? value)
    {
        StringBuilder out_ = new();
        WriteValue(value, out_);
        return out_.ToString();
    }
}
