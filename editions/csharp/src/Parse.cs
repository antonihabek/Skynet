// A JSON parser, for the two things this edition reads: the corpus files, and the `expected` strings inside
// them that the round trip re-serialises.
//
// Deliberately a parser of JSON rather than of canonical JSON. The corpus files are written by
// `tools/corpus-gen/generate.ts` through `JSON.stringify` with indentation, so they carry whitespace,
// unsorted keys and `\uXXXX` escapes for anything this writer would emit as UTF-8 — and a parser that only
// accepted the canonical form could not read the corpus it exists to read.
//
// What it refuses is everything outside JSON proper: a trailing comma, a comment, `NaN`, `Infinity`, a bare
// word, a leading `+`, a leading zero, a control character inside a string. Each of those is a file this
// edition cannot have produced. §3.9's rule applies rather than being softened because the input is a
// checked-in file: all input from outside the process is untrusted, and a corpus path is an argument.
//
// No `System.Text.Json`, no `Utf8JsonReader`, no regex. §31.4 requires byte-identical output, so the number
// conversion and the duplicate-key rule have to be ours rather than whatever the framework does this year —
// and `System.Text.Json` differs from the reference on both: it rejects a lone surrogate in a `\u` escape,
// which a JavaScript string may hold, and its `GetDouble` is not the engine's conversion.

using System;
using System.Globalization;
using System.Text;

namespace Skynet.Os;

/// <summary>Where a parse stopped and why.</summary>
public sealed class ParseException : Exception
{
    public ParseException(string message)
        : base(message)
    {
    }

    public ParseException()
        : base("the document is not JSON")
    {
    }

    public ParseException(string message, Exception inner)
        : base(message, inner)
    {
    }
}

internal sealed class Parse
{
    /// <summary>
    /// Bounds the nesting a document may reach. §7.5: bound recursion on untrusted input. Deeper than any
    /// corpus document and shallow enough that the recursion cannot reach the stack this edition runs on.
    /// </summary>
    private const int MaxDepth = 512;

    private readonly string bytes;
    private int at;
    private int depth;

    private Parse(string text) => bytes = text;

    /// <summary>
    /// Parse <c>text</c>. Trailing whitespace is allowed; trailing anything else is not, so a file holding two
    /// documents fails rather than silently reporting the first.
    /// </summary>
    internal static Value Document(string text)
    {
        ArgumentNullException.ThrowIfNull(text);
        string held = text;
        // A UTF-8 byte-order mark, which an editor on Windows will add to a file it saves. Skipped rather than
        // refused, and only at the start. `File.ReadAllText` already strips one, so this is for a string that
        // did not come from a file.
        if (held.Length > 0 && held[0] == '﻿')
        {
            held = held.Substring(1);
        }
        Parse parser = new(held);
        Value out_ = parser.ValueAt();
        parser.SkipSpace();
        if (!parser.AtEnd)
        {
            throw parser.Fail("trailing bytes after the document");
        }
        return out_;
    }

    private ParseException Fail(string message)
    {
        int line = 1;
        int column = 1;
        for (int scan = 0; scan < Math.Min(at, bytes.Length); scan++)
        {
            if (bytes[scan] == '\n')
            {
                line += 1;
                column = 1;
            }
            else
            {
                column += 1;
            }
        }
        return new ParseException(
            line.ToString(CultureInfo.InvariantCulture) + ":" + column.ToString(CultureInfo.InvariantCulture)
                + ": " + message);
    }

    private bool AtEnd => at >= bytes.Length;

    private char Peek => AtEnd ? '\0' : bytes[at];

    private void SkipSpace()
    {
        while (!AtEnd)
        {
            char ch = bytes[at];
            if (ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r')
            {
                at += 1;
            }
            else
            {
                break;
            }
        }
    }

    private void Literal(string word)
    {
        if (bytes.Length - at < word.Length
            || string.CompareOrdinal(bytes, at, word, 0, word.Length) != 0)
        {
            throw Fail("not a JSON value");
        }
        at += word.Length;
    }

    private int Hex4()
    {
        if (bytes.Length - at < 4)
        {
            throw Fail("a truncated \\u escape");
        }
        int value = 0;
        for (int digit = 0; digit < 4; digit++)
        {
            char ch = bytes[at + digit];
            // ASCII hex only. `char.IsDigit` accepts a fullwidth digit and the Arabic-Indic ones, and
            // `Convert.ToInt32(s, 16)` accepts a leading sign and whitespace; none of those is a hex digit in
            // JSON, so the range is checked here rather than trusted to a helper.
            int held;
            if (ch >= '0' && ch <= '9')
            {
                held = ch - '0';
            }
            else if (ch >= 'a' && ch <= 'f')
            {
                held = ch - 'a' + 10;
            }
            else if (ch >= 'A' && ch <= 'F')
            {
                held = ch - 'A' + 10;
            }
            else
            {
                throw Fail("a \\u escape with a non-hex digit");
            }
            value = (value * 16) + held;
        }
        at += 4;
        return value;
    }

    private string StringBytes()
    {
        if (Peek != '"')
        {
            throw Fail("expected a string");
        }
        at += 1;
        StringBuilder out_ = new();
        while (true)
        {
            if (AtEnd)
            {
                throw Fail("an unterminated string");
            }
            char ch = bytes[at];
            if (ch == '"')
            {
                at += 1;
                break;
            }
            if (ch < 0x20)
            {
                throw Fail("an unescaped control character in a string");
            }
            if (ch != '\\')
            {
                out_.Append(ch);
                at += 1;
                continue;
            }
            at += 1;
            if (AtEnd)
            {
                throw Fail("a trailing backslash in a string");
            }
            char escape = bytes[at];
            at += 1;
            switch (escape)
            {
                case '"': out_.Append('"'); break;
                case '\\': out_.Append('\\'); break;
                case '/': out_.Append('/'); break;
                case 'b': out_.Append('\b'); break;
                case 'f': out_.Append('\f'); break;
                case 'n': out_.Append('\n'); break;
                case 'r': out_.Append('\r'); break;
                case 't': out_.Append('\t'); break;
                case 'u':
                    // One UTF-16 code unit per escape, so a surrogate pair assembles itself: `JSON.stringify`
                    // writes two escapes for a character above the BMP, and appending both units puts the pair
                    // in the `string` exactly as the reference holds it. A lone surrogate survives for the same
                    // reason, which it must, because a JavaScript string can hold one — and which is where
                    // `System.Text.Json` would refuse the document instead.
                    out_.Append((char)Hex4());
                    break;
                default: throw Fail("an unknown escape in a string");
            }
        }
        return out_.ToString();
    }

    private Value Number()
    {
        int start = at;
        if (Peek == '-')
        {
            at += 1;
        }
        // JSON forbids a leading zero before another digit, and accepting one would be accepting a document this
        // edition's writer cannot produce.
        if (Peek == '0')
        {
            at += 1;
            if (Peek >= '0' && Peek <= '9')
            {
                throw Fail("a number with a leading zero");
            }
        }
        else if (Peek >= '1' && Peek <= '9')
        {
            while (Peek >= '0' && Peek <= '9')
            {
                at += 1;
            }
        }
        else
        {
            throw Fail("a number with no digits");
        }
        if (Peek == '.')
        {
            at += 1;
            if (!(Peek >= '0' && Peek <= '9'))
            {
                throw Fail("a number with no digits after the point");
            }
            while (Peek >= '0' && Peek <= '9')
            {
                at += 1;
            }
        }
        if (Peek == 'e' || Peek == 'E')
        {
            at += 1;
            if (Peek == '+' || Peek == '-')
            {
                at += 1;
            }
            if (!(Peek >= '0' && Peek <= '9'))
            {
                throw Fail("a number with no digits in its exponent");
            }
            while (Peek >= '0' && Peek <= '9')
            {
                at += 1;
            }
        }
        // The run has been validated as a JSON number above, so this converts rather than parses — and it is
        // the correctly rounded conversion the reference's `Number(...)` performs. The culture is invariant
        // explicitly: `double.Parse` with the current culture reads `1.5` as fifteen where the decimal
        // separator is a comma, and `InvariantGlobalization` in the project file does not change that default.
        string run = bytes.Substring(start, at - start);
        return Value.Of(double.Parse(run, NumberStyles.Float, CultureInfo.InvariantCulture));
    }

    private Value ArrayValue()
    {
        at += 1;
        Value out_ = Value.Array();
        SkipSpace();
        if (Peek == ']')
        {
            at += 1;
            return out_;
        }
        while (true)
        {
            out_.Push(ValueAt());
            SkipSpace();
            if (Peek == ',')
            {
                at += 1;
                SkipSpace();
                // A trailing comma is the one JSON5 habit a hand-edited corpus file would pick up, and it is
                // named separately so the message says what is wrong rather than "not a value".
                if (Peek == ']')
                {
                    throw Fail("a trailing comma in an array");
                }
                continue;
            }
            if (Peek == ']')
            {
                at += 1;
                return out_;
            }
            throw Fail("expected a comma or a closing bracket");
        }
    }

    private Value ObjectValue()
    {
        at += 1;
        Value out_ = Value.Object();
        SkipSpace();
        if (Peek == '}')
        {
            at += 1;
            return out_;
        }
        while (true)
        {
            SkipSpace();
            string key = StringBytes();
            SkipSpace();
            if (Peek != ':')
            {
                throw Fail("expected a colon after a key");
            }
            at += 1;
            // A duplicate key replaces the earlier one in place, which is what the reference's object literal
            // does.
            out_.Set(key, ValueAt());
            SkipSpace();
            if (Peek == ',')
            {
                at += 1;
                SkipSpace();
                if (Peek == '}')
                {
                    throw Fail("a trailing comma in an object");
                }
                continue;
            }
            if (Peek == '}')
            {
                at += 1;
                return out_;
            }
            throw Fail("expected a comma or a closing brace");
        }
    }

    private Value ValueAt()
    {
        if (depth >= MaxDepth)
        {
            throw Fail("a document nested deeper than this parser will go");
        }
        SkipSpace();
        if (AtEnd)
        {
            throw Fail("the document ended where a value was expected");
        }
        depth += 1;
        try
        {
            switch (Peek)
            {
                case '{': return ObjectValue();
                case '[': return ArrayValue();
                case '"': return Value.Of(StringBytes());
                case 't':
                    Literal("true");
                    return Value.True;
                case 'f':
                    Literal("false");
                    return Value.False;
                case 'n':
                    Literal("null");
                    return Value.Null;
                default: return Number();
            }
        }
        finally
        {
            depth -= 1;
        }
    }
}
