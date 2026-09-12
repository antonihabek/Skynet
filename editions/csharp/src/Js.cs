// The JavaScript semantics the reference relies on, spelled out.
//
// A `string` here is UTF-16, exactly as a JavaScript string is, which removes the representation question every
// other edition has to answer and makes the four differences below easier to walk past:
//
// | The reference calls | .NET's near-equivalent | Why it is not the same |
// |---|---|---|
// | `trim()` | `string.Trim()` | Measured: it does **not** remove U+FEFF and it **does** remove U+0085. ECMAScript's set is the exact inverse on both, and the corpus turns on each of them |
// | `toUpperCase()` | `ToUpperInvariant()` | The *simple* mapping, one code point to one. A sharp s stays a sharp s, and a capital sigma has no final form |
// | `${value}` | `double.ToString()` | Number::toString writes `1e-7` and `1e+21` where canonical form writes them positionally, and `NaN` where canonical form refuses it |
// | `Number(text)` | `double.Parse` | `Number("0x1f")` is 31 and carries no sign; `Number("Infinity")` needs that capital; `Number("")` is zero |
//
// The second row is the one that surprises: .NET's case functions are the simple mapping, like Go's, not the
// full one like Java's. Both are wrong for this purpose and for different reasons, which is why every edition
// carries the tables `tools/unicode-case.ts` reads out of the engine.

using System;
using System.Globalization;
using System.Text;

namespace Skynet.Os;

internal static class Js
{
    internal const string UnicodeVersion = UnicodeCase.UnicodeVersion;

    // ── Whitespace ───────────────────────────────────────────────────────────

    /// <summary>
    /// ECMAScript's WhiteSpace and LineTerminator, which is what <c>trim()</c> removes.
    /// </summary>
    /// <remarks>
    /// Written out as the <i>language's</i> set rather than derived from a Unicode property, because that is
    /// what it is: U+FEFF is whitespace to JavaScript and is not <c>White_Space</c> in Unicode, and U+0085 is
    /// the other way round.
    /// </remarks>
    internal static bool IsSpace(int code) => code switch
    {
        0x0009 or 0x000A or 0x000B or 0x000C or 0x000D or 0x0020 or 0x00A0 or 0x1680
            or 0x2028 or 0x2029 or 0x202F or 0x205F or 0x3000 or 0xFEFF => true,
        // U+2000 to U+200A, the en quad through the hair space.
        _ => code >= 0x2000 && code <= 0x200A,
    };

    internal static string Trim(string text)
    {
        // No character in the whitespace set is above the basic multilingual plane, so the pair handling here
        // is not about finding one — it is so that a lone surrogate is *stepped over* rather than thrown on.
        int start = 0;
        while (start < text.Length)
        {
            int code = CodePointAt(text, start);
            if (!IsSpace(code))
            {
                break;
            }
            start += WidthAt(text, start);
        }
        int end = text.Length;
        while (end > start)
        {
            int back = char.IsLowSurrogate(text[end - 1]) && end - 2 >= start
                && char.IsHighSurrogate(text[end - 2]) ? 2 : 1;
            int code = CodePointAt(text, end - back);
            if (!IsSpace(code))
            {
                break;
            }
            end -= back;
        }
        return text[start..end];
    }

    // ── Case ─────────────────────────────────────────────────────────────────

    /// <summary>Flattened pairs: <c>ranges[2 * i]</c> and <c>ranges[2 * i + 1]</c>.</summary>
    private static bool InRanges(int[] ranges, int code)
    {
        int low = 0;
        int high = ranges.Length / 2;
        while (low < high)
        {
            int middle = low + ((high - low) / 2);
            if (code < ranges[2 * middle])
            {
                high = middle;
            }
            else if (code > ranges[(2 * middle) + 1])
            {
                low = middle + 1;
            }
            else
            {
                return true;
            }
        }
        return false;
    }

    private static bool IsCased(int code) => InRanges(UnicodeCase.casedRanges, code);

    private static bool IsCaseIgnorable(int code) => InRanges(UnicodeCase.caseIgnorableRanges, code);

    /// <summary>
    /// The index of <c>code</c> in an ascending table, or -1 when it maps to itself.
    /// </summary>
    /// <remarks>
    /// A binary search; a table that lost its order would still answer most lookups, which is why the
    /// self-check asserts the order rather than trusting it.
    /// </remarks>
    private static int Lookup(int[] from, int code)
    {
        int low = 0;
        int high = from.Length;
        while (low < high)
        {
            int middle = low + ((high - low) / 2);
            if (code < from[middle])
            {
                high = middle;
            }
            else if (code > from[middle])
            {
                low = middle + 1;
            }
            else
            {
                return middle;
            }
        }
        return -1;
    }

    /// <summary>True when both mapping tables are ascending and the ranges do not overlap.</summary>
    internal static bool TablesSorted()
    {
        for (int at = 1; at < UnicodeCase.upperFrom.Length; at++)
        {
            if (UnicodeCase.upperFrom[at - 1] >= UnicodeCase.upperFrom[at])
            {
                return false;
            }
        }
        for (int at = 1; at < UnicodeCase.lowerFrom.Length; at++)
        {
            if (UnicodeCase.lowerFrom[at - 1] >= UnicodeCase.lowerFrom[at])
            {
                return false;
            }
        }
        for (int at = 1; at < UnicodeCase.casedRanges.Length / 2; at++)
        {
            if (UnicodeCase.casedRanges[(2 * at) - 1] >= UnicodeCase.casedRanges[2 * at])
            {
                return false;
            }
        }
        for (int at = 1; at < UnicodeCase.caseIgnorableRanges.Length / 2; at++)
        {
            if (UnicodeCase.caseIgnorableRanges[(2 * at) - 1] >= UnicodeCase.caseIgnorableRanges[2 * at])
            {
                return false;
            }
        }
        return true;
    }

    /// <summary>
    /// The code point at <c>at</c>, or the lone surrogate itself when there is no pair — which is what the
    /// reference's <c>codePointAt</c> answers, and which <c>char.ConvertToUtf32</c> throws on instead.
    /// </summary>
    /// <remarks>
    /// A JavaScript string is a sequence of UTF-16 code units and may hold an unpaired surrogate, so a corpus
    /// document may too: the parser assembles one <c>\uXXXX</c> escape per code unit precisely so that a lone
    /// one survives. Every walk over a string in this module goes through here, because the alternative is an
    /// exception on an input the oracle answers.
    /// </remarks>
    private static int CodePointAt(string text, int at)
    {
        char unit = text[at];
        if (char.IsHighSurrogate(unit) && at + 1 < text.Length && char.IsLowSurrogate(text[at + 1]))
        {
            return char.ConvertToUtf32(unit, text[at + 1]);
        }
        return unit;
    }

    /// <summary>How many code units <c>CodePointAt</c> consumed at <c>at</c>.</summary>
    private static int WidthAt(string text, int at) =>
        char.IsHighSurrogate(text[at]) && at + 1 < text.Length && char.IsLowSurrogate(text[at + 1]) ? 2 : 1;

    /// <summary>
    /// Append one code point, including a lone surrogate — <c>char.ConvertFromUtf32</c> refuses those, and a
    /// string that held one must come back out holding it.
    /// </summary>
    private static void AppendCodePoint(StringBuilder out_, int code)
    {
        if (code <= 0xFFFF)
        {
            out_.Append((char)code);
            return;
        }
        out_.Append(char.ConvertFromUtf32(code));
    }

    private static void AppendMapping(StringBuilder out_, int[] offsets, int[] flat, int index)
    {
        for (int at = offsets[index]; at < offsets[index + 1]; at++)
        {
            AppendCodePoint(out_, flat[at]);
        }
    }

    internal static string Upper(string text)
    {
        StringBuilder out_ = new();
        int at = 0;
        while (at < text.Length)
        {
            int code = CodePointAt(text, at);
            at += WidthAt(text, at);
            int index = Lookup(UnicodeCase.upperFrom, code);
            if (index < 0)
            {
                AppendCodePoint(out_, code);
            }
            else
            {
                AppendMapping(out_, UnicodeCase.upperOffsets, UnicodeCase.upperFlat, index);
            }
        }
        return out_.ToString();
    }

    private const int CapitalSigma = 0x03A3;
    private const int SmallSigma = 0x03C3;
    private const int FinalSigma = 0x03C2;

    /// <summary>
    /// Unicode's Final_Sigma condition, which is the one context rule <c>toLowerCase</c> applies outside the
    /// Turkish and Lithuanian tailorings — and which .NET's <c>ToLowerInvariant</c> does not apply at all.
    /// </summary>
    /// <remarks>
    /// Before: a cased character, skipping case-ignorable ones. After: <i>not</i> a cased character, skipping
    /// case-ignorable ones. A character that is both cased and case-ignorable is <b>skipped</b>, because the
    /// condition skips Case_Ignorable first — and that is the class <c>tools/unicode-case.ts</c> picks
    /// representatives for, so the vectors are what say whether this reading is the engine's.
    /// </remarks>
    private static bool ScanCased(string text, int from, bool forward)
    {
        if (forward)
        {
            int at = from;
            while (at < text.Length)
            {
                int code = CodePointAt(text, at);
                at += WidthAt(text, at);
                if (IsCaseIgnorable(code))
                {
                    continue;
                }
                return IsCased(code);
            }
            return false;
        }
        int back = from;
        while (back > 0)
        {
            int width = char.IsLowSurrogate(text[back - 1]) && back - 2 >= 0
                && char.IsHighSurrogate(text[back - 2]) ? 2 : 1;
            int code = CodePointAt(text, back - width);
            back -= width;
            if (IsCaseIgnorable(code))
            {
                continue;
            }
            return IsCased(code);
        }
        return false;
    }

    internal static string Lower(string text)
    {
        StringBuilder out_ = new();
        int at = 0;
        while (at < text.Length)
        {
            int start = at;
            int code = CodePointAt(text, at);
            at += WidthAt(text, at);
            if (code == CapitalSigma)
            {
                bool before = ScanCased(text, start, false);
                bool after = ScanCased(text, at, true);
                AppendCodePoint(out_, before && !after ? FinalSigma : SmallSigma);
                continue;
            }
            int index = Lookup(UnicodeCase.lowerFrom, code);
            if (index < 0)
            {
                AppendCodePoint(out_, code);
            }
            else
            {
                AppendMapping(out_, UnicodeCase.lowerOffsets, UnicodeCase.lowerFlat, index);
            }
        }
        return out_.ToString();
    }

    // ── Numbers and ToString ─────────────────────────────────────────────────

    /// <summary>
    /// ECMAScript's Number::toString at radix 10 — <c>String(value)</c>, not the canonical writer.
    /// </summary>
    /// <remarks>
    /// Stated over three integers: <c>s</c> with <c>k</c> digits and no trailing zero, and <c>n</c> where
    /// <c>s * 10^(n-k)</c> is the value. <c>ToString("E16")</c> gives a fixed-width scientific rendering and
    /// the shortest round-trip digits come from <c>"R"</c>, so the two are combined: <c>"R"</c> for the digits,
    /// and its own exponent for <c>n</c>.
    /// </remarks>
    internal static string NumberToString(double number)
    {
        if (double.IsNaN(number))
        {
            return "NaN";
        }
        if (number == 0.0)
        {
            return "0";
        }
        if (double.IsInfinity(number))
        {
            return number < 0 ? "-Infinity" : "Infinity";
        }

        double magnitude = Math.Abs(number);
        // `"E16"` is scientific with a fixed 17 significant digits, which is never shorter than the shortest
        // round-trip; `"R"` is the shortest but may be positional. So the digits are taken from `"R"` and the
        // decimal exponent is computed from them rather than read off a format string.
        string shortest = magnitude.ToString("R", CultureInfo.InvariantCulture);
        string digits;
        int n;
        int marker = shortest.IndexOf('E', StringComparison.Ordinal);
        if (marker >= 0)
        {
            string mantissa = shortest[..marker];
            int exponent = int.Parse(shortest[(marker + 1)..], CultureInfo.InvariantCulture);
            int pointAt = mantissa.IndexOf('.', StringComparison.Ordinal);
            digits = mantissa.Replace(".", string.Empty, StringComparison.Ordinal);
            // The mantissa has one digit before the point, so its own exponent is `n - 1` directly.
            n = exponent + 1;
            _ = pointAt;
        }
        else
        {
            int pointAt = shortest.IndexOf('.', StringComparison.Ordinal);
            string whole = pointAt < 0 ? shortest : shortest[..pointAt];
            string fraction = pointAt < 0 ? string.Empty : shortest[(pointAt + 1)..];
            digits = whole + fraction;
            // Leading zeros in a value below one are not significant digits; each one lowers `n` by a place.
            int leading = 0;
            while (leading < digits.Length - 1 && digits[leading] == '0')
            {
                leading += 1;
            }
            digits = digits[leading..];
            n = whole.Length - leading;
        }
        // ECMAScript's `s` carries no trailing zero, and the rules below count digits.
        int k = digits.Length;
        while (k > 1 && digits[k - 1] == '0')
        {
            k -= 1;
        }
        digits = digits[..k];

        StringBuilder out_ = new();
        if (number < 0)
        {
            out_.Append('-');
        }
        if (k <= n && n <= 21)
        {
            out_.Append(digits).Append('0', n - k);
        }
        else if (n > 0 && n <= 21)
        {
            out_.Append(digits, 0, n).Append('.').Append(digits[n..]);
        }
        else if (n > -6 && n <= 0)
        {
            out_.Append("0.").Append('0', -n).Append(digits);
        }
        else
        {
            // Exponential — which is the whole reason the canonical writer does not use this function.
            out_.Append(digits[0]);
            if (k > 1)
            {
                out_.Append('.').Append(digits[1..]);
            }
            out_.Append('e').Append(n - 1 >= 0 ? "+" : "-")
                .Append(Math.Abs(n - 1).ToString(CultureInfo.InvariantCulture));
        }
        return out_.ToString();
    }

    /// <summary>
    /// ECMAScript's ToString over a parsed JSON value — what a template literal does to whatever it is handed.
    /// </summary>
    internal static string ToStringOf(Value? value, bool present)
    {
        if (!present || value is null)
        {
            return "undefined";
        }
        switch (value.ValueKind)
        {
            case Kind.Null:
                return "null";
            case Kind.Bool:
                return Value.AsBool(value, false) ? "true" : "false";
            case Kind.Number:
                return NumberToString(value.Number);
            case Kind.String:
                return value.Text;
            case Kind.Array:
                {
                    // Array.prototype.join: a null or undefined element is the empty string rather than "null",
                    // which is the one place ToString's answer for null is not the word.
                    StringBuilder out_ = new();
                    for (int at = 0; at < Value.Length(value); at++)
                    {
                        if (at > 0)
                        {
                            out_.Append(',');
                        }
                        Value? element = Value.At(value, at);
                        if (Value.Nullish(element))
                        {
                            continue;
                        }
                        out_.Append(ToStringOf(element, true));
                    }
                    return out_.ToString();
                }
            default:
                return "[object Object]";
        }
    }

    /// <summary>
    /// ECMAScript's ToNumber over a string — <c>Number(text)</c>, which is not <c>double.Parse</c>.
    /// </summary>
    /// <remarks>
    /// <c>Number("0x1f")</c> is 31, because a string may hold a non-decimal integer literal — and it carries
    /// <b>no sign</b> in the grammar, so <c>-0x10</c> is NaN. <c>Number("Infinity")</c> needs that capital. And
    /// <c>Number("")</c> is <i>zero</i>, which is why the reference guards with its own blank test.
    /// </remarks>
    internal static double StringToNumber(string text)
    {
        string trimmed = Trim(text);
        if (trimmed.Length == 0)
        {
            return 0;
        }

        if (trimmed.Length > 2 && trimmed[0] == '0')
        {
            int radix = trimmed[1] switch
            {
                'x' or 'X' => 16,
                'o' or 'O' => 8,
                'b' or 'B' => 2,
                _ => 0,
            };
            if (radix != 0)
            {
                double held = 0;
                for (int at = 2; at < trimmed.Length; at++)
                {
                    char ch = trimmed[at];
                    int digit;
                    if (ch >= '0' && ch <= '9')
                    {
                        digit = ch - '0';
                    }
                    else if (ch >= 'a' && ch <= 'z')
                    {
                        digit = ch - 'a' + 10;
                    }
                    else if (ch >= 'A' && ch <= 'Z')
                    {
                        digit = ch - 'A' + 10;
                    }
                    else
                    {
                        return double.NaN;
                    }
                    if (digit >= radix)
                    {
                        return double.NaN;
                    }
                    held = (held * radix) + digit;
                }
                return held;
            }
        }

        int scan = 0;
        double sign = 1;
        if (trimmed[scan] == '+' || trimmed[scan] == '-')
        {
            sign = trimmed[scan] == '-' ? -1 : 1;
            scan += 1;
        }
        if (string.Equals(trimmed[scan..], "Infinity", StringComparison.Ordinal))
        {
            return sign * double.PositiveInfinity;
        }

        // StrUnsignedDecimalLiteral, validated here and converted by `double.Parse` — the validation is what
        // keeps `Parse`'s wider grammar out.
        int integerDigits = 0;
        while (scan < trimmed.Length && trimmed[scan] >= '0' && trimmed[scan] <= '9')
        {
            scan += 1;
            integerDigits += 1;
        }
        int fractionDigits = 0;
        if (scan < trimmed.Length && trimmed[scan] == '.')
        {
            scan += 1;
            while (scan < trimmed.Length && trimmed[scan] >= '0' && trimmed[scan] <= '9')
            {
                scan += 1;
                fractionDigits += 1;
            }
        }
        if (integerDigits == 0 && fractionDigits == 0)
        {
            return double.NaN;
        }
        if (scan < trimmed.Length && (trimmed[scan] == 'e' || trimmed[scan] == 'E'))
        {
            scan += 1;
            if (scan < trimmed.Length && (trimmed[scan] == '+' || trimmed[scan] == '-'))
            {
                scan += 1;
            }
            int exponentDigits = 0;
            while (scan < trimmed.Length && trimmed[scan] >= '0' && trimmed[scan] <= '9')
            {
                scan += 1;
                exponentDigits += 1;
            }
            if (exponentDigits == 0)
            {
                return double.NaN;
            }
        }
        if (scan != trimmed.Length)
        {
            return double.NaN;
        }
        return double.Parse(trimmed, CultureInfo.InvariantCulture);
    }
}
